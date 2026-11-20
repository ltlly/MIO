use std::fs;

use super::{io::read_preferences, tests::location, *};

#[test]
fn user_tls_certificates_preserve_other_preferences_and_require_global_cas() {
    let (_directory, path) = location();
    let mut first = PreferencesStore::new(Some(path.clone()));
    let mut second = PreferencesStore::new(Some(path.clone()));
    let certificates = vec![
        mio_proxy::MitmAuthority::generate()
            .unwrap()
            .certificate_der(),
    ];
    let saved = first.set_tls_certificates(0, certificates.clone()).unwrap();
    assert_eq!(saved.revision, 1);
    let before = fs::read(&path).unwrap();
    assert!(second.set_tls_certificates(0, Vec::new()).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(matches!(
        second
            .effective_upstream_tls(&crate::UpstreamTlsSettings::default())
            .unwrap(),
        mio_proxy::UpstreamTls::System
    ));
    assert_eq!(second.query().preferences, saved);
    let closed = first
        .set(
            1,
            RunningAttackClose::Continue,
            FinishedAttackClose::KeepMemory,
        )
        .unwrap();
    assert_eq!(closed.custom_ca_ders, certificates);
    let startup = first
        .set_startup(2, StartupDefaultConfig::Project, true)
        .unwrap();
    assert_eq!(startup.custom_ca_ders, certificates);
    let recent = first
        .record_recent("TLS project", PathBuf::from("/tmp/tls.mio"), 10)
        .unwrap();
    assert_eq!(recent.custom_ca_ders, certificates);
    assert_eq!(recent.running, closed.running);
    assert_eq!(recent.finished, closed.finished);
    assert_eq!(recent.startup, startup.startup);
    let mut reopened = PreferencesStore::new(Some(path.clone()));
    assert_eq!(reopened.query().preferences, recent);
    let cleared = reopened.set_tls_certificates(4, Vec::new()).unwrap();
    assert_eq!(cleared.revision, 5);
    assert!(cleared.custom_ca_ders.is_empty());
    assert_eq!(cleared.recent_projects, recent.recent_projects);
    assert_eq!(cleared.startup, recent.startup);
    assert!(!fs::read_to_string(path).unwrap().contains("custom_ca_ders"));
}

#[test]
fn user_tls_malformed_duplicate_and_over_limit_certificates_do_not_replace_preferences() {
    let (_directory, path) = location();
    let mut store = PreferencesStore::new(Some(path.clone()));
    let certificate = mio_proxy::MitmAuthority::generate()
        .unwrap()
        .certificate_der();
    let confirmed = store
        .set_tls_certificates(0, vec![certificate.clone()])
        .unwrap();
    let before = fs::read(&path).unwrap();
    for invalid in [
        vec![Vec::new()],
        vec![vec![0x30, 0]],
        vec![certificate.clone(), certificate.clone()],
        vec![certificate.clone(); MAX_TLS_CERTIFICATES + 1],
        vec![vec![0x30; MAX_TLS_CERTIFICATE_BYTES + 1]],
    ] {
        assert!(store.set_tls_certificates(1, invalid.clone()).is_err());
        assert_eq!(store.query().preferences, confirmed);
        assert_eq!(fs::read(&path).unwrap(), before);
        let invalid_profile = UserPreferences {
            custom_ca_ders: invalid,
            ..confirmed.clone()
        };
        fs::write(&path, serde_json::to_vec(&invalid_profile).unwrap()).unwrap();
        assert!(store.query().load_error.is_some());
        assert!(store.set_tls_certificates(1, Vec::new()).is_err());
        assert_eq!(
            fs::read(&path).unwrap(),
            serde_json::to_vec(&invalid_profile).unwrap()
        );
        fs::write(&path, &before).unwrap();
    }
    for invalid in [
        br#"{"version":1,"revision":1,"running":"Ask","finished":"Ask","custom_ca_ders":null}"#
            .as_slice(),
        br#"{"version":1,"revision":1,"running":"Ask","finished":"Ask","custom_ca_ders":[],"custom_ca_ders":[]}"#,
        br#"{"version":1,"revision":1,"running":"Ask","finished":"Ask","custom_ca_ders":[],"unknown_tls":true}"#,
    ] {
        fs::write(&path, invalid).unwrap();
        assert!(store.query().load_error.is_some());
        assert!(store.set_tls_certificates(1, Vec::new()).is_err());
        assert_eq!(fs::read(&path).unwrap(), invalid);
    }
}

#[test]
fn user_tls_certificate_budget_does_not_expand_non_certificate_preferences() {
    let (_directory, path) = location();
    let preferences = UserPreferences {
        recent_projects: (0..20)
            .map(|index| RecentProject {
                name: format!("{}-{index}", "n".repeat(512)),
                path: PathBuf::from(format!("/tmp/{}-{index}.mio", "p".repeat(2300))),
                last_accessed_unix_ms: index,
            })
            .collect(),
        ..UserPreferences::default()
    };
    let original = serde_json::to_vec(&preferences).unwrap();
    assert!(original.len() as u64 <= MAX_BYTES);
    fs::write(&path, &original).unwrap();
    let mut store = PreferencesStore::new(Some(path.clone()));
    assert_eq!(store.query().preferences, preferences);
    let certificates = (0..8)
        .map(|_| {
            mio_proxy::MitmAuthority::generate()
                .unwrap()
                .certificate_der()
        })
        .collect::<Vec<_>>();
    let saved = store.set_tls_certificates(0, certificates).unwrap();
    let encoded = fs::read(&path).unwrap();
    assert!(encoded.len() as u64 > MAX_BYTES);
    assert!(encoded.len() as u64 <= MAX_FILE_BYTES);
    assert_eq!(read_preferences(&path).unwrap(), saved);
    let mut oversized = saved.clone();
    oversized
        .recent_projects
        .extend((20..32).map(|index| RecentProject {
            name: format!("{}-{index}", "n".repeat(512)),
            path: PathBuf::from(format!("/tmp/{}-{index}.mio", "p".repeat(2300))),
            last_accessed_unix_ms: index,
        }));
    let oversized = serde_json::to_vec(&oversized).unwrap();
    fs::write(&path, &oversized).unwrap();
    assert!(read_preferences(&path).is_err());
    assert!(store.set_tls_certificates(1, Vec::new()).is_err());
    assert_eq!(fs::read(&path).unwrap(), oversized);
    let mut padded = serde_json::to_vec(&UserPreferences::default()).unwrap();
    padded.resize(MAX_BYTES as usize + 1, b' ');
    fs::write(&path, &padded).unwrap();
    assert!(read_preferences(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), padded);
}

#[test]
fn user_tls_confirmed_snapshot_preserves_legacy_project_roots_and_rejects_load_error() {
    let (_directory, path) = location();
    let mut store = PreferencesStore::new(Some(path.clone()));
    let user_certificate = mio_proxy::MitmAuthority::generate()
        .unwrap()
        .certificate_der();
    store
        .set_tls_certificates(0, vec![user_certificate.clone()])
        .unwrap();
    let project_certificate = mio_proxy::MitmAuthority::generate()
        .unwrap()
        .certificate_der();
    let legacy = crate::UpstreamTlsSettings {
        custom_ca_ders: vec![project_certificate.clone()],
        include_system_roots: false,
        insecure_skip_verify: false,
        ..crate::UpstreamTlsSettings::default()
    };
    assert!(matches!(
        store.effective_upstream_tls(&legacy).unwrap(),
        mio_proxy::UpstreamTls::CustomRoots { certificates_der, include_system: false }
            if certificates_der == legacy.custom_ca_ders
    ));
    let default_project = crate::UpstreamTlsSettings::default();
    assert!(matches!(
        store.effective_upstream_tls(&default_project).unwrap(),
        mio_proxy::UpstreamTls::CustomRoots { certificates_der, include_system: true }
            if certificates_der == vec![user_certificate.clone()]
    ));
    fs::write(&path, b"broken user preferences").unwrap();
    assert!(matches!(
        store.effective_upstream_tls(&default_project).unwrap(),
        mio_proxy::UpstreamTls::CustomRoots { certificates_der, include_system: true }
            if certificates_der == vec![user_certificate]
    ));
    assert!(store.query().load_error.is_some());
    assert!(store.effective_upstream_tls(&default_project).is_err());
    assert!(matches!(
        store.effective_upstream_tls(&legacy).unwrap(),
        mio_proxy::UpstreamTls::CustomRoots { certificates_der, include_system: false }
            if certificates_der == vec![project_certificate]
    ));
    let invalid_project = crate::UpstreamTlsSettings {
        custom_ca_ders: vec![vec![0x30, 0]],
        include_system_roots: false,
        insecure_skip_verify: false,
        ..crate::UpstreamTlsSettings::default()
    };
    assert!(store.effective_upstream_tls(&invalid_project).is_err());
    assert!(store.set_tls_certificates(1, Vec::new()).is_err());
    assert_eq!(fs::read(path).unwrap(), b"broken user preferences");
}
