use super::*;

#[test]
fn evaluate_grep_config_matches_expressions_and_extracts_delimited_values() {
    use crate::{IntruderGrepConfig, IntruderGrepExtractRule};

    let long_utf8 = "中".repeat(300);
    let response = format!(
        "HTTP/1.1 200 OK\r\nX-Token: [secret-123]\r\n\r\nHello Admin_User <val>{long_utf8}</val> <unclosed>tail"
    );
    let mut config = IntruderGrepConfig {
        match_expressions: vec!["admin_user".into(), "Admin_User".into(), "missing".into()],
        match_case_sensitive: false,
        extract_rules: vec![
            IntruderGrepExtractRule {
                start_delimiter: "[".into(),
                end_delimiter: "]".into(),
            },
            IntruderGrepExtractRule {
                start_delimiter: "<val>".into(),
                end_delimiter: "</val>".into(),
            },
            IntruderGrepExtractRule {
                start_delimiter: "<unclosed>".into(),
                end_delimiter: "</unclosed>".into(),
            },
        ],
        grep_payloads: false,
        grep_payloads_case_sensitive: false,
    };

    let (matched_ci, extracted) = evaluate_grep_config(response.as_bytes(), &config);
    assert_eq!(matched_ci, vec!["admin_user", "Admin_User"]);
    let expected_truncated = format!("{}…", "中".repeat(256));
    assert_eq!(
        extracted,
        vec![
            "[..]: secret-123".to_string(),
            format!("<val>..</val>: {expected_truncated}"),
        ]
    );

    config.match_case_sensitive = true;
    let (matched_cs, _) = evaluate_grep_config(response.as_bytes(), &config);
    assert_eq!(matched_cs, vec!["Admin_User"]);
}

#[test]
fn evaluate_grep_config_preserves_non_utf8_bytes_and_marks_truncation() {
    use crate::{IntruderGrepConfig, IntruderGrepExtractRule};

    let mut raw = Vec::from(&b"HTTP/1.1 200 OK\r\n\r\nprefix "[..]);
    raw.extend_from_slice(&[0xff, 0xfe, 0x00]);
    raw.extend_from_slice(b" BeArEr_Token <bin>");
    raw.extend_from_slice(&[b'A', 0xff, 0x00, 0xfe]);
    raw.extend_from_slice("中".as_bytes());
    raw.extend_from_slice(b"</bin> <long>");
    raw.extend(std::iter::repeat_n(0xff, 260));
    raw.extend_from_slice(b"</long> <huge>");
    raw.extend(std::iter::repeat_n(0xff, 512 * 1024));
    raw.extend_from_slice(b"</huge>");

    let config = IntruderGrepConfig {
        match_expressions: vec!["bearer_token".into(), "BEARER_TOKEN".into()],
        match_case_sensitive: false,
        extract_rules: vec![
            IntruderGrepExtractRule {
                start_delimiter: "<bin>".into(),
                end_delimiter: "</bin>".into(),
            },
            IntruderGrepExtractRule {
                start_delimiter: "<long>".into(),
                end_delimiter: "</long>".into(),
            },
            IntruderGrepExtractRule {
                start_delimiter: "<huge>".into(),
                end_delimiter: "</huge>".into(),
            },
        ],
        grep_payloads: false,
        grep_payloads_case_sensitive: false,
    };
    let (matched, extracted) = evaluate_grep_config(&raw, &config);
    assert_eq!(matched, vec!["bearer_token", "BEARER_TOKEN"]);
    assert_eq!(extracted[0], "<bin>..</bin>: A\\xff\\x00\\xfe中");
    assert!(!extracted[0].contains('\u{fffd}'));
    assert_eq!(
        extracted[1],
        format!("<long>..</long>: {}…", "\\xff".repeat(256))
    );
    assert_eq!(
        extracted[2],
        format!("<huge>..</huge>: {}…", "\\xff".repeat(256))
    );

    let mut oversized = Vec::with_capacity(MAX_REQUEST_BYTES + 64);
    oversized.extend_from_slice(b"HTTP/1.1 200 OK\r\n\r\n<bin>ok</bin>");
    oversized.resize(MAX_REQUEST_BYTES + 16, b'x');
    let (_, oversized_extracted) = evaluate_grep_config(&oversized, &config);
    assert_eq!(
        oversized_extracted,
        vec!["<bin>..</bin>: ok [响应超限截断]".to_string()]
    );
}

#[test]
fn evaluate_grep_matches_supports_bounded_regex_prefix_and_find_echoed_inputs() {
    use crate::{
        IntruderGrepConfig,
        intruder::grep::{evaluate_grep_matches, find_echoed_inputs},
    };

    let response = b"HTTP/1.1 200 OK\r\nX-Trace: req-4096\r\n\r\necho=<probe&1> User_Id=42";
    let config = IntruderGrepConfig {
        match_expressions: vec![
            "re:req-\\d{4}".into(),
            "re:user_id=\\d+".into(),
            "re:[invalid".into(),
            format!("re:a{{{}}}", 200_000),
            "re:".into(),
        ],
        match_case_sensitive: false,
        extract_rules: Vec::new(),
        grep_payloads: false,
        grep_payloads_case_sensitive: false,
    };
    let (matched_ci, _) = evaluate_grep_config(response, &config);
    assert_eq!(matched_ci, vec!["re:req-\\d{4}", "re:user_id=\\d+"]);

    let matched_cs = evaluate_grep_matches(response, &config.match_expressions, true);
    assert_eq!(matched_cs, vec!["re:req-\\d{4}"]);

    let inputs = vec![
        String::new(),
        "<probe&1>".into(),
        "missing".into(),
        "<probe&1>".into(),
        "req-4096".into(),
    ];
    assert_eq!(
        find_echoed_inputs(response, &inputs),
        vec!["<probe&1>".to_string(), "req-4096".to_string()]
    );
}

#[test]
fn evaluate_grep_row_extracts_regex_groups_and_detects_payload_hits() {
    use crate::{IntruderGrepConfig, IntruderGrepExtractRule, intruder::evaluate_grep_row};

    let response =
        b"HTTP/1.1 200 OK\r\nX-Session: sid-9876\r\n\r\nuser=AdminPayload123 token=[tok_abc]";
    let config = IntruderGrepConfig {
        match_expressions: vec!["sid-9876".into()],
        match_case_sensitive: true,
        extract_rules: vec![
            IntruderGrepExtractRule::regex(r"X-Session:\s*(\S+)"),
            IntruderGrepExtractRule::delimited("[", "]"),
        ],
        grep_payloads: true,
        grep_payloads_case_sensitive: false,
    };
    let payloads: &[&[u8]] = &[b"adminpayload123", b"missing_payload"];
    let (matches, extracts, hits) = evaluate_grep_row(response, &config, payloads);
    assert_eq!(matches, vec!["sid-9876".to_string()]);
    assert_eq!(
        extracts,
        vec![
            (r"re:X-Session:\s*(\S+)".to_string(), "sid-9876".to_string()),
            ("[..]".to_string(), "tok_abc".to_string()),
        ]
    );
    assert_eq!(hits, vec![true, false]);

    let mut strict_config = config.clone();
    strict_config.grep_payloads_case_sensitive = true;
    let (_, _, strict_hits) = evaluate_grep_row(response, &strict_config, payloads);
    assert_eq!(strict_hits, vec![false, false]);
}
