#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::{
    fs::{self, OpenOptions},
    path::Path,
};

use super::*;

pub(super) fn location() -> (tempfile::TempDir, PathBuf) {
    #[cfg(unix)]
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    #[cfg(not(unix))]
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .canonicalize()
        .unwrap()
        .join("preferences.json");
    (directory, path)
}

#[test]
fn preferences_persist_reopen_and_reject_another_instances_stale_revision() {
    let (_directory, path) = location();
    let mut first = PreferencesStore::new(Some(path.clone()));
    let mut second = PreferencesStore::new(Some(path.clone()));
    assert_eq!(first.query().preferences, UserPreferences::default());
    assert!(!path.exists(), "query does not create a preferences file");
    let saved = first
        .set(
            0,
            RunningAttackClose::Continue,
            FinishedAttackClose::KeepMemory,
        )
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert!(
        second
            .set(0, RunningAttackClose::Delete, FinishedAttackClose::Delete)
            .is_err()
    );
    assert_eq!(second.query().preferences, saved);
    assert_eq!(
        PreferencesStore::new(Some(path.clone()))
            .query()
            .preferences,
        saved
    );
    let before = fs::read(&path).unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path.with_file_name("preferences.lock"))
        .unwrap();
    lock.lock().unwrap();
    assert!(
        second
            .set(1, RunningAttackClose::Ask, FinishedAttackClose::Ask)
            .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn malformed_unknown_and_oversized_preferences_stay_untouched_with_safe_ask() {
    let (_directory, path) = location();
    for bytes in [
        b"broken".to_vec(),
        br#"{"version":1,"revision":2,"running":"Execute","finished":"Ask"}"#.to_vec(),
        br#"{"version":1,"revision":2,"running":"Ask","finished":"Ask","unknown_top":true}"#
            .to_vec(),
        br#"{"version":1,"revision":2,"running":"Ask","finished":"Ask","startup":{"default_config":"Defaults","extra":1}}"#.to_vec(),
        br#"{"version":1,"revision":2,"running":"Ask","finished":"Ask","recent_projects":[{"name":"a","path":"/tmp/a.mio","last_accessed_unix_ms":1,"extra":1}]}"#.to_vec(),
        br#"{"version":2,"revision":2,"running":"Ask","finished":"Ask"}"#.to_vec(),
        vec![b' '; MAX_BYTES as usize + 1],
    ] {
        fs::write(&path, &bytes).unwrap();
        let mut store = PreferencesStore::new(Some(path.clone()));
        let state = store.query();
        assert_eq!(state.preferences, UserPreferences::default());
        assert!(state.load_error.is_some());
        assert!(
            store
                .set(
                    0,
                    RunningAttackClose::Continue,
                    FinishedAttackClose::SaveProject
                )
                .is_err()
        );
        assert!(
            store
                .set_startup(0, StartupDefaultConfig::Project, true)
                .is_err()
        );
        assert!(
            store
                .record_recent("demo", PathBuf::from("/tmp/demo.mio"), 100)
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn legacy_json_and_closing_settings_preserve_startup_and_recent_projects_with_cas() {
    let (_directory, path) = location();
    let legacy = br#"{"version":1,"revision":3,"running":"Continue","finished":"KeepMemory"}"#;
    fs::write(&path, legacy).unwrap();

    let mut first = PreferencesStore::new(Some(path.clone()));
    let mut second = PreferencesStore::new(Some(path.clone()));
    let loaded = first.query();
    assert!(loaded.load_error.is_none());
    assert_eq!(loaded.preferences.revision, 3);
    assert_eq!(loaded.preferences.running, RunningAttackClose::Continue);
    assert_eq!(loaded.preferences.finished, FinishedAttackClose::KeepMemory);
    assert_eq!(loaded.preferences.startup, StartupPreferences::default());
    assert!(loaded.preferences.recent_projects.is_empty());

    let saved_legacy_shape = first
        .set(
            3,
            RunningAttackClose::Delete,
            FinishedAttackClose::SaveProject,
        )
        .unwrap();
    assert_eq!(
        String::from_utf8(fs::read(&path).unwrap()).unwrap(),
        r#"{"version":1,"revision":4,"running":"Delete","finished":"SaveProject"}"#
    );
    assert_eq!(saved_legacy_shape.revision, 4);

    let config_path = PathBuf::from("/tmp/mio-custom-config.json");
    let with_startup = first
        .set_startup(
            4,
            StartupDefaultConfig::ConfigFile {
                path: config_path.clone(),
            },
            true,
        )
        .unwrap();
    assert_eq!(with_startup.revision, 5);
    assert_eq!(with_startup.running, RunningAttackClose::Delete);
    assert_eq!(with_startup.finished, FinishedAttackClose::SaveProject);
    assert_eq!(
        with_startup.startup,
        StartupPreferences {
            default_config: Some(StartupDefaultConfig::ConfigFile { path: config_path }),
            disable_extensions: true,
        }
    );

    assert!(
        second
            .set_startup(3, StartupDefaultConfig::Defaults, false)
            .is_err(),
        "stale expected revision must reject startup preference write without overwriting"
    );

    let recorded = second
        .record_recent("Alpha", PathBuf::from("/tmp/alpha.mio"), 1_700_000_000_000)
        .unwrap();
    assert_eq!(recorded.revision, 6);
    assert_eq!(recorded.running, RunningAttackClose::Delete);
    assert_eq!(recorded.finished, FinishedAttackClose::SaveProject);
    assert_eq!(recorded.startup, with_startup.startup);
    assert_eq!(
        recorded.recent_projects,
        vec![RecentProject {
            name: "Alpha".into(),
            path: PathBuf::from("/tmp/alpha.mio"),
            last_accessed_unix_ms: 1_700_000_000_000,
        }]
    );

    assert!(
        first
            .set(5, RunningAttackClose::Ask, FinishedAttackClose::Ask)
            .is_err(),
        "record_recent advances revision so stale closing preference write fails CAS"
    );
    let refreshed = first.query().preferences;
    assert_eq!(refreshed, recorded);

    let after_closing_save = first
        .set(6, RunningAttackClose::Ask, FinishedAttackClose::Ask)
        .unwrap();
    assert_eq!(after_closing_save.revision, 7);
    assert_eq!(after_closing_save.startup, with_startup.startup);
    assert_eq!(after_closing_save.recent_projects, recorded.recent_projects);

    let explicit_defaults = first
        .set_startup(7, StartupDefaultConfig::Defaults, false)
        .unwrap();
    assert_eq!(explicit_defaults.revision, 8);
    assert_eq!(
        explicit_defaults.startup.default_config,
        Some(StartupDefaultConfig::Defaults)
    );
    assert!(
        !explicit_defaults.startup.is_default(),
        "explicitly remembered Defaults must not collapse into unremembered default"
    );
    let raw_json = String::from_utf8(fs::read(&path).unwrap()).unwrap();
    assert!(
        raw_json.contains(r#""default_config":"Defaults""#),
        "explicit Defaults must be serialized in preferences.json: {raw_json}"
    );
}

#[test]
fn recent_projects_dedup_by_path_sort_stably_and_cap_at_thirty_two() {
    let (_directory, path) = location();
    let mut store = PreferencesStore::new(Some(path.clone()));

    for index in 0..36 {
        let project_path = PathBuf::from(format!("/tmp/project-{index:02}.mio"));
        store
            .record_recent(
                format!("Project {index:02}"),
                project_path,
                1_000 + index as i64 * 10,
            )
            .unwrap();
    }
    let capped = store.query().preferences;
    assert_eq!(capped.recent_projects.len(), MAX_RECENT_PROJECTS);
    assert_eq!(
        capped.recent_projects.first().unwrap().path,
        PathBuf::from("/tmp/project-35.mio")
    );
    assert_eq!(
        capped.recent_projects.last().unwrap().path,
        PathBuf::from("/tmp/project-04.mio")
    );

    let reopened_same_path = store
        .record_recent("", PathBuf::from("/tmp/project-10.mio"), 2_000)
        .unwrap();
    assert_eq!(
        reopened_same_path.recent_projects.len(),
        MAX_RECENT_PROJECTS
    );
    assert_eq!(
        reopened_same_path.recent_projects[0],
        RecentProject {
            name: "project-10".into(),
            path: PathBuf::from("/tmp/project-10.mio"),
            last_accessed_unix_ms: 2_000,
        }
    );
    assert_eq!(
        reopened_same_path
            .recent_projects
            .iter()
            .filter(|item| item.path == Path::new("/tmp/project-10.mio"))
            .count(),
        1
    );

    let tie_break = store
        .record_recent("TieWinner", PathBuf::from("/tmp/tie-winner.mio"), 2_000)
        .unwrap();
    assert_eq!(
        tie_break.recent_projects[0].path,
        PathBuf::from("/tmp/tie-winner.mio")
    );
    assert_eq!(
        tie_break.recent_projects[1].path,
        PathBuf::from("/tmp/project-10.mio")
    );
}

#[cfg(unix)]
#[test]
fn symlink_and_fifo_preferences_are_rejected_without_following_or_blocking() {
    let (_directory, path) = location();
    let target = path.with_file_name("untouched");
    fs::write(&target, b"unrelated").unwrap();
    symlink(&target, &path).unwrap();
    let mut store = PreferencesStore::new(Some(path.clone()));
    assert!(store.query().load_error.is_some());
    assert!(
        store
            .set(0, RunningAttackClose::Ask, FinishedAttackClose::Ask)
            .is_err()
    );
    assert_eq!(fs::read(&target).unwrap(), b"unrelated");
    fs::remove_file(&path).unwrap();
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    assert!(store.query().load_error.is_some());
    assert!(
        store
            .set(0, RunningAttackClose::Ask, FinishedAttackClose::Ask)
            .is_err()
    );
}
