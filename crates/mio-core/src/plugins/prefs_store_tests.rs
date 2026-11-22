use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::prefs_store::PluginPreferencesStore;
use crate::{Header, HttpRequest, HttpVersion, RepeaterSettings};

#[test]
fn plugin_preferences_persist_across_reopen_isolate_namespaces_and_enforce_limits() {
    let dir = TempDir::new().expect("temp dir");
    let canonical_root = std::fs::canonicalize(dir.path()).expect("canonicalize temp dir");
    let prefs_path = canonical_root.join("prefs").join("plugin-preferences.json");
    let cancel = CancellationToken::new();

    let mut store = PluginPreferencesStore::new(Some(prefs_path.clone()));
    assert_eq!(store.get("mio.alpha", "endpoint").unwrap(), None);
    store
        .set("mio.alpha", "endpoint", b"http://127.0.0.1:9001", &cancel)
        .unwrap();
    store
        .set("mio.beta", "endpoint", b"http://127.0.0.1:9002", &cancel)
        .unwrap();

    let mut reopened = PluginPreferencesStore::new(Some(prefs_path));
    assert_eq!(
        reopened.get("mio.alpha", "endpoint").unwrap().as_deref(),
        Some(&b"http://127.0.0.1:9001"[..])
    );
    assert_eq!(
        reopened.get("mio.beta", "endpoint").unwrap().as_deref(),
        Some(&b"http://127.0.0.1:9002"[..])
    );

    // Empty value deletes only the targeted key in that plugin namespace.
    reopened.set("mio.alpha", "endpoint", b"", &cancel).unwrap();
    assert_eq!(reopened.get("mio.alpha", "endpoint").unwrap(), None);
    assert!(reopened.get("mio.beta", "endpoint").unwrap().is_some());

    // Oversized value (>64 KiB) and cancelled token are rejected cleanly.
    let oversized = vec![b'x'; 65 * 1024];
    assert!(
        reopened
            .set("mio.alpha", "big", &oversized, &cancel)
            .is_err()
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        reopened
            .set("mio.alpha", "cancelled", b"1", &cancelled)
            .is_err()
    );

    let user_pref = canonical_root.join("profile").join("preferences.json");
    let mut derived = PluginPreferencesStore::from_env_or_user_preferences(Some(&user_pref));
    derived
        .set("mio.gamma", "token", b"persisted-across-projects", &cancel)
        .unwrap();
    let sibling_json = canonical_root
        .join("profile")
        .join("plugin-preferences.json");
    assert!(sibling_json.is_file());
    let on_disk = std::fs::read_to_string(&sibling_json).unwrap();
    assert!(on_disk.contains("mio.gamma"));
    let expected_b64 = crate::codec_util::base64_encode(b"persisted-across-projects");
    assert!(
        on_disk.contains(&format!("\"{expected_b64}\"")),
        "values must be persisted as Base64 strings, got: {on_disk}"
    );
    assert!(
        !on_disk.contains("[112,101,"),
        "values must not be persisted as JSON byte arrays"
    );

    // Cross-instance updates and legacy v0.2.2 byte-array entries must be visible without stale cache.
    assert_eq!(
        derived.get("mio.gamma", "token").unwrap().as_deref(),
        Some(&b"persisted-across-projects"[..])
    );
    let mut second_instance = PluginPreferencesStore::from_env_or_user_preferences(Some(&user_pref));
    second_instance
        .set("mio.gamma", "token", b"updated-by-peer", &cancel)
        .unwrap();
    assert_eq!(
        derived.get("mio.gamma", "token").unwrap().as_deref(),
        Some(&b"updated-by-peer"[..]),
        "peer writes must be observed by other instances"
    );

    std::fs::write(
        &sibling_json,
        r#"{"version":1,"plugins":{"mio.legacy":{"k":[104,101,108,108,111]}}}"#,
    )
    .unwrap();
    assert_eq!(
        derived.get("mio.legacy", "k").unwrap().as_deref(),
        Some(&b"hello"[..])
    );
    derived
        .set("mio.gamma", "token", b"updated-token", &cancel)
        .unwrap();
    let migrated_json = std::fs::read_to_string(&sibling_json).unwrap();
    assert!(migrated_json.contains(&crate::codec_util::base64_encode(b"hello")));
    assert_eq!(
        derived.get("mio.gamma", "token").unwrap().as_deref(),
        Some(&b"updated-token"[..])
    );
}

#[test]
fn repeater_protocol_options_strip_h2_connection_headers_and_enforce_http1_close() {
    let mut h2_req = HttpRequest {
        method: "GET".into(),
        url: "https://127.0.0.1:8443/api".into(),
        version: HttpVersion::Http11,
        headers: vec![
            Header {
                name: "Host".into(),
                value: b"127.0.0.1:8443".to_vec(),
            },
            Header {
                name: "Connection".into(),
                value: b"keep-alive, X-Hop-Temp".to_vec(),
            },
            Header {
                name: "Keep-Alive".into(),
                value: b"timeout=5".to_vec(),
            },
            Header {
                name: "X-Hop-Temp".into(),
                value: b"strip-me".to_vec(),
            },
            Header {
                name: "X-Keep".into(),
                value: b"1".to_vec(),
            },
        ],
        body: Vec::new(),
        trailers: Vec::new(),
    };
    let settings = RepeaterSettings {
        strip_connection_headers_over_http2: true,
        enforce_http1_connection_close: true,
        auto_follow_redirections_max_hops: 99,
        ..RepeaterSettings::default()
    };
    assert_eq!(settings.effective_max_redirect_hops(), 20);
    settings.apply_protocol_options(&mut h2_req, HttpVersion::Http2);
    assert_eq!(h2_req.version, HttpVersion::Http2);
    let names: Vec<&str> = h2_req.headers.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, vec!["Host", "X-Keep"]);

    let mut h1_req = h2_req.clone();
    settings.apply_protocol_options(&mut h1_req, HttpVersion::Http11);
    assert_eq!(h1_req.version, HttpVersion::Http11);
    assert_eq!(
        h1_req
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("connection"))
            .map(|h| h.value.as_slice()),
        Some(&b"close"[..])
    );
}
