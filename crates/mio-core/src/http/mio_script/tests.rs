use mio_plugin_api::{
    FlowRecord, FlowState, FlowSummary, Header, HttpRequest, HttpResponse, HttpVersion,
    TrafficSource, Uuid,
};
use tokio_util::sync::CancellationToken;

use super::engine::{BoundedLruCache, MIO_SCRIPT_AST_CACHE_LIMIT, ast_cache_contains, shared_engine_build_count};
use super::highlight::ScriptRequirements;
use super::*;
use crate::codec_util;

#[cfg(test)]
mod keyword_range_tests {
    use super::*;

    #[test]
    fn script_keyword_ranges_lexes_keywords_and_skips_strings_comments_and_unicode_crlf() {
        assert!(script_keyword_ranges("").is_empty());
        let simple = "return true;";
        let slices: Vec<&str> = script_keyword_ranges(simple)
            .into_iter()
            .map(|r| &simple[r])
            .collect();
        assert_eq!(slices, vec!["return", "true"]);

        let complex = concat!(
            "// comment with return true false\r\n",
            "/* block /* nested return */ false */\r\n",
            "let 备注 = \"return true\";\r\n",
            "let tpl = `prefix return ${if true { false } else { true }} suffix return`;\r\n",
            "if !false { return true; } else { return ("
        );
        let ranges = script_keyword_ranges(complex);
        let matched: Vec<&str> = ranges.iter().map(|r| &complex[r.clone()]).collect();
        assert_eq!(
            matched,
            vec![
                "let", "let", "if", "true", "false", "else", "true", "if", "false", "return",
                "true", "else", "return"
            ]
        );
        for w in ranges.windows(2) {
            assert!(w[0].end <= w[1].start);
        }
    }

    #[test]
    fn script_keyword_ranges_skips_raw_strings_char_literals_and_nested_template_maps() {
        let script = concat!(
            "let r = ##\"return #\"true\"# false\"##;\n",
            "let ch = '\\'';\n",
            "let 备注return = 1;\n",
            "let msg = `a ${let m = #{ k: if true { 1 } else { 2 } }; m} return`;\n",
            "return false;"
        );
        let ranges = script_keyword_ranges(script);
        let matched: Vec<&str> = ranges.iter().map(|r| &script[r.clone()]).collect();
        assert_eq!(
            matched,
            vec!["let", "let", "let", "let", "let", "if", "true", "else", "return", "false"]
        );
    }
}

#[cfg(test)]
mod custom_action_mutation_tests {
    use super::*;

    fn sample_request() -> HttpRequest {
        HttpRequest {
            method: "GET".into(),
            url: "https://api.example.invalid/v1/profile?tab=info&debug=1".into(),
            version: HttpVersion::Http11,
            headers: vec![
                Header {
                    name: "Host".into(),
                    value: b"api.example.invalid".to_vec(),
                },
                Header {
                    name: "Cookie".into(),
                    value: b"session=secret123; theme=dark".to_vec(),
                },
                Header {
                    name: "Authorization".into(),
                    value: b"Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyMSJ9.sig".to_vec(),
                },
                Header {
                    name: "If-None-Match".into(),
                    value: b"\"etag-1\"".to_vec(),
                },
            ],
            body: Vec::new(),
            trailers: Vec::new(),
        }
    }

    #[test]
    fn custom_action_strips_all_cookies_and_returns_updated_request() {
        let req = sample_request();
        let outcome = eval_custom_action_with_mutation(
            r#"
            remove_all_cookies(request);
            print("removed");
            "#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(outcome.output, "removed");
        let updated = outcome
            .updated_request
            .expect("Cookie removal must produce updated_request");
        assert!(
            !updated
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("cookie"))
        );
        assert!(
            updated
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("authorization"))
        );
    }

    #[test]
    fn custom_action_cookie_header_and_query_helpers_roundtrip_cleanly() {
        let req = sample_request();
        let outcome = eval_custom_action_with_mutation(
            r#"
            request.removeCookie("session");
            request.setCookie("role", "guest");
            request.removeHeaders(["Authorization", "If-None-Match"]);
            request.removeQueryParam("debug");
            request.setQueryParam("_cb", "42");
            print(base64url_decode(base64url_encode("hello+world/=")));
            "#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(outcome.output, "hello+world/=");
        let updated = outcome.updated_request.unwrap();
        let cookie_val = updated
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("cookie"))
            .map(|h| String::from_utf8_lossy(&h.value).into_owned());
        assert_eq!(cookie_val.as_deref(), Some("theme=dark; role=guest"));
        assert!(
            !updated
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("authorization")
                    || h.name.eq_ignore_ascii_case("if-none-match"))
        );
        assert_eq!(
            updated.url,
            "https://api.example.invalid/v1/profile?tab=info&_cb=42"
        );
    }

    #[test]
    fn readonly_custom_action_leaves_updated_request_none() {
        let req = sample_request();
        let outcome = eval_custom_action_with_mutation(
            r#"print(request.method + " " + request.url);"#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(
            outcome.output,
            "GET https://api.example.invalid/v1/profile?tab=info&debug=1"
        );
        assert_eq!(outcome.updated_request, None);
    }

    #[test]
    fn script_module_imports_are_disabled() {
        let req = sample_request();
        let err = eval_custom_action_with_mutation(
            r#"import "foo" as f; print("unreachable");"#,
            &req,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(!err.is_empty());
    }
}

#[cfg(test)]
mod expanded_security_helpers_tests {
    use super::*;

    fn json_post_request() -> HttpRequest {
        let body = br#"{"user":{"role":"guest","tags":["read"]},"active":true}"#.to_vec();
        HttpRequest {
            method: "POST".into(),
            url: "https://api.example.invalid/v1/users".into(),
            version: HttpVersion::Http11,
            headers: vec![
                Header {
                    name: "Host".into(),
                    value: b"api.example.invalid".to_vec(),
                },
                Header {
                    name: "Content-Type".into(),
                    value: b"application/json".to_vec(),
                },
                Header {
                    name: "Cookie".into(),
                    value: b"sid=abc123; prefs=compact".to_vec(),
                },
                Header {
                    name: "Authorization".into(),
                    value: b"Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJhZG1pbiJ9.sig_ok".to_vec(),
                },
                Header {
                    name: "Content-Length".into(),
                    value: body.len().to_string().into_bytes(),
                },
            ],
            body,
            trailers: Vec::new(),
        }
    }

    #[test]
    fn json_helpers_parse_query_stringify_and_mutate_request_body() {
        let req = json_post_request();
        let outcome = eval_custom_action_with_mutation(
            r#"
            let obj = json_parse(request.body);
            let tag0 = json_get(request.body, "user.tags.0");
            let missing = utilities.jsonGet(request.body, "user.missing.0");
            let compact = utilities.jsonStringify(obj);
            let pretty = utilities.jsonStringifyPretty(#{ ok: true });
            request.setJsonField("user.role", "admin");
            request.setJsonField("meta.source", "mio");
            request.setJsonField("user.tags.0", "write");
            print(obj.user.role + "|" + tag0 + "|" + (missing == ()) + "|" + compact.contains("guest") + "|" + pretty.contains("\n"));
            "#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(outcome.output, "guest|read|true|true|true");
        let updated = outcome.updated_request.expect("JSON mutation updates body");
        let parsed: serde_json::Value = serde_json::from_slice(&updated.body).unwrap();
        assert_eq!(parsed["user"]["role"], "admin");
        assert_eq!(parsed["user"]["tags"][0], "write");
        assert_eq!(parsed["meta"]["source"], "mio");
        let cl = updated
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-length"))
            .map(|h| String::from_utf8_lossy(&h.value).into_owned());
        assert_eq!(cl.as_deref(), Some(updated.body.len().to_string().as_str()));
    }

    #[test]
    fn hex_and_html_encode_decode_work_and_validate() {
        let req = json_post_request();
        let outcome = eval_custom_action_with_mutation(
            r#"
            let hx = hex_encode("Hi!");
            let dec = hex_decode("48 69 21");
            let ent = html_encode("<a href=\"x\">'&'</a>");
            let plain = utilities.htmlDecode(ent + " &#65; &#x42; &apos; &unknown;");
            print(hx + "|" + dec + "|" + utilities.hexEncode("A") + "|" + utilities.hexDecode("41") + "|" + plain);
            "#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(
            outcome.output,
            "486921|Hi!|41|A|<a href=\"x\">'&'</a> A B ' &unknown;"
        );
        assert_eq!(outcome.updated_request, None);

        assert!(eval_custom_action_with_mutation(r#"hex_decode("abc");"#, &req, None).is_err());
        assert!(eval_custom_action_with_mutation(r#"hex_decode("zz");"#, &req, None).is_err());
    }

    #[test]
    fn hmac_sha256_matches_rfc4231_and_long_key_consistency() {
        let req = json_post_request();
        let outcome = eval_custom_action_with_mutation(
            r#"
            let rfc = hmac_sha256_hex("Jefe", "what do ya want for nothing?");
            let b64 = utilities.hmacSha256Base64("Jefe", "what do ya want for nothing?");
            let b64u = utilities.hmacSha256Base64Url("Jefe", "what do ya want for nothing?");
            let long_key = "k0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
            let long_hex = utilities.hmacSha256Hex(long_key, "payload");
            let long_b64 = hmac_sha256_base64(long_key, "payload");
            print(rfc + "|" + b64 + "|" + b64u + "|" + long_hex.len() + "|" + (long_b64 == hmac_sha256_base64(long_key, "payload")));
            "#,
            &req,
            None,
        )
        .unwrap();
        let expected_b64 = codec_util::base64_encode(&[
            0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e, 0x6a, 0x04, 0x24, 0x26, 0x08, 0x95,
            0x75, 0xc7, 0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83, 0x9d, 0xec, 0x58, 0xb9,
            0x64, 0xec, 0x38, 0x43,
        ]);
        let expected_b64u = codec_util::base64url_encode(&[
            0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e, 0x6a, 0x04, 0x24, 0x26, 0x08, 0x95,
            0x75, 0xc7, 0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83, 0x9d, 0xec, 0x58, 0xb9,
            0x64, 0xec, 0x38, 0x43,
        ]);
        assert_eq!(
            outcome.output,
            format!(
                "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843|{expected_b64}|{expected_b64u}|64|true"
            )
        );
    }

    #[test]
    fn jwt_decode_handles_bearer_and_raw_tokens() {
        let req = json_post_request();
        let outcome = eval_custom_action_with_mutation(
            r#"
            let auth = request.headerValue("Authorization");
            let decoded = jwt_decode(auth);
            let raw_tok = auth.sub_string(7);
            let decoded2 = utilities.jwtDecode(raw_tok);
            print(decoded.header.alg + "|" + decoded.payload.sub + "|" + decoded.signature + "|" + decoded2.payload.role + "|" + decoded.raw_payload.contains("user1"));
            "#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(outcome.output, "HS256|user1|sig_ok|admin|true");
        assert!(
            eval_custom_action_with_mutation(r#"jwt_decode("bad.token");"#, &req, None).is_err()
        );
    }

    #[test]
    fn cookie_and_form_helpers_read_and_mutate_request() {
        let mut req = json_post_request();
        req.headers
            .retain(|h| !h.name.eq_ignore_ascii_case("content-type"));
        req.headers.push(Header {
            name: "Content-Type".into(),
            value: b"application/x-www-form-urlencoded".to_vec(),
        });
        req.body = b"user=alice&role=user&debug=1".to_vec();

        let outcome = eval_custom_action_with_mutation(
            r#"
            let cookies = request.parseCookies();
            let before_role = request.formParam("role");
            request.setFormParam("role", "admin");
            request.setFormParam("next", "/dash");
            request.removeFormParam("debug");
            print(cookies.sid + "|" + cookies.prefs + "|" + before_role);
            "#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(outcome.output, "abc123|compact|user");
        let updated = outcome
            .updated_request
            .expect("form mutation updates request");
        assert_eq!(
            String::from_utf8_lossy(&updated.body),
            "user=alice&role=admin&next=%2Fdash"
        );
        let cl = updated
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-length"))
            .map(|h| String::from_utf8_lossy(&h.value).into_owned());
        assert_eq!(cl.as_deref(), Some(updated.body.len().to_string().as_str()));
    }
}

#[cfg(test)]
mod refactor_guarantees_tests {
    use super::*;

    fn neutral_request() -> HttpRequest {
        HttpRequest {
            method: "GET".into(),
            url: "https://api.example.invalid/v1/items?page=1".into(),
            version: HttpVersion::Http11,
            headers: vec![
                Header {
                    name: "Host".into(),
                    value: b"api.example.invalid".to_vec(),
                },
                Header {
                    name: "X-Tag".into(),
                    value: b"one".to_vec(),
                },
                Header {
                    name: "X-Tag".into(),
                    value: b"two".to_vec(),
                },
            ],
            body: Vec::new(),
            trailers: Vec::new(),
        }
    }

    fn neutral_response() -> HttpResponse {
        HttpResponse {
            version: HttpVersion::Http11,
            status: 200,
            reason_phrase: Some(b"OK".to_vec()),
            headers: vec![
                Header {
                    name: "Content-Type".into(),
                    value: b"application/json".to_vec(),
                },
                Header {
                    name: "Set-Cookie".into(),
                    value: b"sid=a1; Path=/".to_vec(),
                },
                Header {
                    name: "Set-Cookie".into(),
                    value: b"prefs=b2; HttpOnly".to_vec(),
                },
            ],
            body: br#"{"ok":true}"#.to_vec(),
            trailers: Vec::new(),
        }
    }

    #[test]
    fn utilities_methods_are_scoped_to_script_utilities_and_rejected_on_maps() {
        let req = neutral_request();
        let outcome = eval_custom_action_with_mutation(
            r#"print(type_of(utilities) + "|" + utilities.sha256Hex("abc"));"#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(
            outcome.output,
            "Utilities|ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let filter_ok = eval_http_filter_script_input(
            r#"utilities.sha256Hex("abc") == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad""#,
            &FilterScriptInput {
                method: "GET",
                url: "https://api.example.invalid/v1/items",
                status: Some(200),
                length: 11,
                request_raw: None,
                response_raw: None,
                notes: "",
                highlight: HighlightFilterState::Boolean(false),
                listener_port: Some(8080),
                in_scope: true,
                cancel: None,
            },
        )
        .unwrap();
        assert!(filter_ok);

        assert!(eval_custom_action_with_mutation(r#"#{}.sha256Hex("abc");"#, &req, None).is_err());
        assert!(
            eval_custom_action_with_mutation(r#"request.sha256Hex("abc");"#, &req, None).is_err()
        );
        assert!(
            eval_custom_action_with_mutation(r#"#{}.base64Encode("abc");"#, &req, None).is_err()
        );
    }

    #[test]
    fn header_literally_named_headers_works_without_container_confusion() {
        let mut req = neutral_request();
        req.headers.push(Header {
            name: "headers".into(),
            value: b"literal-val".to_vec(),
        });

        let raw_req = b"GET /v1/items?page=1 HTTP/1.1\r\nHost: api.example.invalid\r\nheaders: literal-val\r\n\r\n";
        let matched = eval_http_filter_script_input(
            r#"
                header_value(request, "headers") == "literal-val"
                    && request.headerValue("headers") == "literal-val"
                    && request.hasHeader("headers")
                    && request.headers["headers"] == "literal-val"
                    && header_value(request.headers, "headers") == "literal-val"
            "#,
            &FilterScriptInput {
                method: "GET",
                url: "https://api.example.invalid/v1/items?page=1",
                status: Some(200),
                length: 0,
                request_raw: Some(raw_req),
                response_raw: None,
                notes: "",
                highlight: HighlightFilterState::Boolean(false),
                listener_port: None,
                in_scope: false,
                cancel: None,
            },
        )
        .unwrap();
        assert!(matched);

        let updated_outcome = eval_custom_action_with_mutation(
            r#"
            let plain = #{ headers: "plain-val", Host: "api.example.invalid" };
            let plain_before = header_value(plain, "headers");
            set_header(plain, "headers", "plain-updated");
            let plain_after = header_value(plain, "headers");
            remove_header(plain, "headers");
            request.set_header("headers", "updated-val");
            print(plain_before + "|" + plain_after + "|" + has_header(plain, "headers"));
            "#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(updated_outcome.output, "plain-val|plain-updated|false");
        let updated = updated_outcome.updated_request.unwrap();
        assert!(updated.headers.iter().any(|h| {
            h.name.eq_ignore_ascii_case("headers") && h.value == b"updated-val"
        }));
        assert!(updated.headers.iter().any(|h| {
            h.name.eq_ignore_ascii_case("host") && h.value == b"api.example.invalid"
        }));

        let removed_outcome = eval_custom_action_with_mutation(
            r#"request.remove_header("headers");"#,
            &req,
            None,
        )
        .unwrap();
        let removed_req = removed_outcome.updated_request.unwrap();
        assert!(
            !removed_req
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("headers"))
        );
        assert!(
            removed_req
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("host"))
        );

        assert!(
            eval_custom_action_with_mutation(
                r#"let bad = #{ headers: 42 }; header_value(bad, "Host");"#,
                &req,
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn duplicate_headers_such_as_multiple_set_cookie_are_preserved() {
        let req = neutral_request();
        let resp = neutral_response();
        let mut record = FlowRecord {
            summary: FlowSummary {
                id: Uuid::nil(),
                revision: 1,
                source: TrafficSource::Proxy,
                method: req.method.clone(),
                url: req.url.clone(),
                status: Some(resp.status),
                state: FlowState::Complete,
                request_bytes: req.body.len() as u64,
                response_bytes: resp.body.len() as u64,
                started_unix_ms: 0,
                elapsed_ms: 1,
            },
            request: req.clone(),
            response: Some(resp.clone()),
        };

        let changed = execute_http_match_replace_script(
            r#"
            response.headers["X-Frame-Options"] = "DENY";
            add_header(response, "Set-Cookie", "extra=c3; Secure");
            response
            "#,
            &mut record,
            true,
        )
        .unwrap();
        assert!(changed);
        let cookies: Vec<String> = record
            .response
            .as_ref()
            .unwrap()
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("set-cookie"))
            .map(|h| String::from_utf8_lossy(&h.value).into_owned())
            .collect();
        assert_eq!(
            cookies,
            vec!["sid=a1; Path=/", "prefs=b2; HttpOnly", "extra=c3; Secure"]
        );
        assert!(record.response.as_ref().unwrap().headers.iter().any(|h| {
            h.name.eq_ignore_ascii_case("x-frame-options") && h.value == b"DENY"
        }));

        let raw_resp =
            b"HTTP/1.1 200 OK\r\nSet-Cookie: sid=a1; Path=/\r\nSet-Cookie: prefs=b2; HttpOnly\r\n\r\nok";
        let filter_ok = eval_http_filter_script_input(
            r#"
                let vals = response.headerValues("Set-Cookie");
                vals.len() == 2 && vals[0] == "sid=a1; Path=/" && vals[1] == "prefs=b2; HttpOnly"
            "#,
            &FilterScriptInput {
                method: "GET",
                url: "https://api.example.invalid/v1/items",
                status: Some(200),
                length: 2,
                request_raw: None,
                response_raw: Some(raw_resp),
                notes: "",
                highlight: HighlightFilterState::Boolean(false),
                listener_port: None,
                in_scope: false,
                cancel: None,
            },
        )
        .unwrap();
        assert!(filter_ok);

        let req_outcome = eval_custom_action_with_mutation(
            r#"request.set_header("X-Extra", "yes");"#,
            &req,
            None,
        )
        .unwrap();
        let tags: Vec<String> = req_outcome
            .updated_request
            .unwrap()
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("x-tag"))
            .map(|h| String::from_utf8_lossy(&h.value).into_owned())
            .collect();
        assert_eq!(tags, vec!["one", "two"]);
    }

    #[test]
    fn custom_action_reuses_shared_engine_and_isolates_print_capture() {
        let req = neutral_request();

        assert!(
            eval_http_filter_script_input(
                r#"request.method == "GET""#,
                &FilterScriptInput {
                    method: "GET",
                    url: "https://api.example.invalid/v1/items",
                    status: Some(200),
                    length: 0,
                    request_raw: None,
                    response_raw: None,
                    notes: "",
                    highlight: HighlightFilterState::Boolean(false),
                    listener_port: None,
                    in_scope: false,
                    cancel: None,
                },
            )
            .unwrap()
        );
        let builds_after_filter = shared_engine_build_count();
        assert_eq!(builds_after_filter, 1);

        let first =
            eval_custom_action_with_mutation(r#"print("first-line");"#, &req, None).unwrap();
        assert_eq!(first.output, "first-line");

        assert!(
            eval_custom_action_with_mutation(
                r#"print("leaked-before-error"); throw "boom";"#,
                &req,
                None,
            )
            .is_err()
        );

        let second =
            eval_custom_action_with_mutation(r#"print("second-line");"#, &req, None).unwrap();
        assert_eq!(second.output, "second-line");
        assert_eq!(shared_engine_build_count(), 1);
    }

    #[test]
    fn bounded_lru_evicts_least_recently_used_entry_only() {
        let mut cache = BoundedLruCache::new(3);
        cache.insert("a".to_owned(), 1);
        cache.insert("b".to_owned(), 2);
        cache.insert("c".to_owned(), 3);
        assert_eq!(cache.get("a"), Some(1));
        cache.insert("d".to_owned(), 4);

        assert_eq!(cache.len(), 3);
        assert!(cache.contains_key("a"));
        assert!(!cache.contains_key("b"));
        assert!(cache.contains_key("c"));
        assert!(cache.contains_key("d"));

        let first_script = "request.method == \"GET\" && 0 == 0";
        validate_filter_script(first_script).unwrap();
        for idx in 1..=MIO_SCRIPT_AST_CACHE_LIMIT {
            if idx == 2 {
                let _ = validate_filter_script(first_script);
            }
            let script = format!("request.method == \"GET\" && {idx} == {idx}");
            validate_filter_script(&script).unwrap();
        }
        assert!(ast_cache_contains(first_script));
        let evicted_second = "request.method == \"GET\" && 1 == 1";
        assert!(!ast_cache_contains(evicted_second));
    }

    #[test]
    fn cancel_hook_reports_generic_script_cancelled_message() {
        let pre_cancelled = CancellationToken::new();
        pre_cancelled.cancel();
        let err = eval_http_filter_script_input(
            "true",
            &FilterScriptInput {
                method: "GET",
                url: "https://api.example.invalid/",
                status: Some(200),
                length: 0,
                request_raw: None,
                response_raw: None,
                notes: "",
                highlight: HighlightFilterState::Boolean(false),
                listener_port: None,
                in_scope: false,
                cancel: Some(&pre_cancelled),
            },
        )
        .unwrap_err()
        .to_string();
        assert_eq!(err, "脚本执行已取消");

        let mid_cancel = CancellationToken::new();
        mid_cancel.cancel();
        let compiled = super::engine::compile_cached("let i = 0; while i < 1000 { i += 1; } true").unwrap();
        let mut scope = rhai::Scope::new();
        let raw_err = super::engine::with_shared_engine(|engine| {
            let _guard = super::engine::ActiveCancelGuard::set(Some(mid_cancel));
            engine
                .eval_ast_with_scope::<bool>(&mut scope, compiled.ast.as_ref())
                .unwrap_err()
        });
        let formatted = super::engine::format_eval_error(raw_err, "脚本失败").to_string();
        assert_eq!(formatted, "脚本执行已取消");
        assert!(!formatted.contains("HTTP 历史查询已取消"));
    }

    #[test]
    fn script_requirements_skip_raw_unless_needed() {
        assert_eq!(
            ScriptRequirements::analyze(
                r#"request.method == "POST" && response.status == 200 && response.body.contains("secret")"#
            ),
            ScriptRequirements {
                needs_raw: false,
                needs_search_text: false,
                needs_request_response: false,
            }
        );
        assert_eq!(
            ScriptRequirements::analyze(r#"request.raw.contains("POST")"#),
            ScriptRequirements {
                needs_raw: true,
                needs_search_text: false,
                needs_request_response: false,
            }
        );
        assert_eq!(
            ScriptRequirements::analyze(r#"requestResponse.contains("hello", false)"#),
            ScriptRequirements {
                needs_raw: true,
                needs_search_text: true,
                needs_request_response: true,
            }
        );
        assert_eq!(
            ScriptRequirements::analyze(r#"regex_matches(search_text, "token")"#),
            ScriptRequirements {
                needs_raw: true,
                needs_search_text: true,
                needs_request_response: false,
            }
        );
    }
}
