use super::*;

#[test]
fn cancellation_before_publication_cleans_both_temporary_directories() {
    for phase in [ExtImportPhase::Archive, ExtImportPhase::Publish] {
        let directory = tempfile::tempdir().unwrap();
        let (path, original) = fixture(directory.path(), 4096);
        let destination = directory.path().join("cancelled.mio");
        let cancel = CancellationToken::new();
        let (outcome, cleanup) = convert(
            inspected(path.clone()),
            destination.clone(),
            &cancel,
            &mut |current, completed, _| {
                if current == phase && (phase == ExtImportPhase::Publish || completed != 0) {
                    cancel.cancel();
                }
            },
        );
        assert!(cleanup.is_none());
        assert!(matches!(outcome, ExtJobOutcome::Cancelled), "{outcome:?}");
        assert!(!destination.exists());
        assert_eq!(directory_names(directory.path()), vec!["synthetic.archive"]);
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn cancellation_after_publication_reports_the_created_project() {
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = fixture(directory.path(), 72);
    let destination = directory.path().join("created.mio");
    let cancel = CancellationToken::new();
    let (outcome, cleanup) = convert(
        inspected(path),
        destination.clone(),
        &cancel,
        &mut |phase, completed, _| {
            if phase == ExtImportPhase::Publish && completed == 1 {
                cancel.cancel();
            }
        },
    );
    assert!(cleanup.is_none());
    assert!(matches!(outcome, ExtJobOutcome::Imported { .. }));
    assert!(
        ProjectStore::open(&destination)
            .unwrap()
            .ext_sources()
            .unwrap()[0]
            .report
            .complete_archive
    );
    assert!(
        directory_names(directory.path())
            .iter()
            .all(|name| !name.starts_with(".mio-ext-"))
    );
}

#[cfg(unix)]
#[test]
fn cancellation_does_not_acknowledge_cleanup_failure_and_retry_reclaims_stage_directory() {
    let directory = tempfile::tempdir().unwrap();
    let (source, bytes) = fixture(directory.path(), 4096);
    let destination = directory.path().join("cancelled.mio");
    let original_permissions = fs::metadata(directory.path()).unwrap().permissions();
    let cancel = CancellationToken::new();
    let (outcome, cleanup) = convert(
        inspected(source.clone()),
        destination.clone(),
        &cancel,
        &mut |phase, completed, _| {
            if phase == ExtImportPhase::Archive && completed != 0 {
                fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500)).unwrap();
                cancel.cancel();
            }
        },
    );
    fs::set_permissions(directory.path(), original_permissions).unwrap();
    assert!(matches!(
        outcome,
        ExtJobOutcome::Failed {
            created_path: None,
            ..
        }
    ));
    let mut cleanup = cleanup.expect("failed cleanup keeps retryable ownership");
    assert_eq!(cleanup.directories.len(), 1);
    assert_eq!(directory_names(directory.path()).len(), 2);
    cleanup.retry().unwrap();
    assert!(cleanup.is_empty());
    cleanup.retry().unwrap();
    assert_eq!(directory_names(directory.path()), vec!["synthetic.archive"]);
    assert!(!destination.exists());
    assert_eq!(fs::read(source).unwrap(), bytes);
}

#[cfg(unix)]
#[test]
fn publication_survives_cleanup_errors_and_retry_reclaims_stage_directory() {
    let directory = tempfile::tempdir().unwrap();
    let (source, bytes) = fixture(directory.path(), 72);
    let destination = directory.path().join("created.mio");
    let original_permissions = fs::metadata(directory.path()).unwrap().permissions();
    let (outcome, cleanup) = convert(
        inspected(source.clone()),
        destination.clone(),
        &CancellationToken::new(),
        &mut |phase, completed, _| {
            if phase == ExtImportPhase::Publish && completed == 1 {
                fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500)).unwrap();
            }
        },
    );
    fs::set_permissions(directory.path(), original_permissions).unwrap();
    assert!(matches!(
        outcome,
        ExtJobOutcome::Failed { created_path: Some(ref path), .. } if *path == destination
    ));
    let mut cleanup = cleanup.expect("cleanup failure stays owned");
    assert_eq!(cleanup.directories.len(), 1);
    cleanup.retry().unwrap();
    assert!(cleanup.is_empty());
    assert!(
        directory_names(directory.path())
            .iter()
            .all(|name| !name.starts_with(".mio-ext-"))
    );
    let project = ProjectStore::open(&destination).unwrap();
    assert!(project.ext_sources().unwrap()[0].report.complete_archive);
    project.close().unwrap();
    assert_eq!(fs::read(source).unwrap(), bytes);
}
