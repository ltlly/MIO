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
fn cancellation_does_not_acknowledge_cleanup_failure_and_retry_reclaims_both_directories() {
    let directory = tempfile::tempdir().unwrap();
    let (source, bytes) = fixture(directory.path(), 4096);
    let destination = directory.path().join("cancelled.mio");
    let original_permissions = fs::metadata(directory.path()).unwrap().permissions();
    let cancel = CancellationToken::new();
    let (outcome, cleanup) = convert(
        inspected_with_snapshot(source.clone()),
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
    assert_eq!(cleanup.directories.len(), 2);
    assert_eq!(directory_names(directory.path()).len(), 3);
    cleanup.retry().unwrap();
    assert!(cleanup.is_empty());
    cleanup.retry().unwrap();
    assert_eq!(directory_names(directory.path()), vec!["synthetic.archive"]);
    assert!(!destination.exists());
    assert_eq!(fs::read(source).unwrap(), bytes);
}

#[test]
fn publication_survives_cleanup_errors_and_replaced_directories_are_never_removed() {
    let directory = tempfile::tempdir().unwrap();
    let (source, bytes) = fixture(directory.path(), 72);
    let inspection = inspected_with_snapshot(source.clone());
    let snapshot = inspection.snapshot.as_ref().unwrap().path().to_owned();
    let destination = directory.path().join("created.mio");
    let mut replaced = Vec::new();
    let (outcome, cleanup) = convert(
        inspection,
        destination.clone(),
        &CancellationToken::new(),
        &mut |phase, completed, _| {
            if phase != ExtImportPhase::Publish || completed != 1 {
                return;
            }
            let stage = fs::read_dir(directory.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(".mio-ext-import-")
                })
                .unwrap();
            for (index, path) in [stage, snapshot.clone()].into_iter().enumerate() {
                let held = directory.path().join(format!("held-{index}"));
                fs::rename(&path, &held).unwrap();
                fs::create_dir(&path).unwrap();
                fs::write(path.join("other-data"), b"keep this replacement").unwrap();
                replaced.push((path, held));
            }
        },
    );
    assert!(matches!(
        outcome,
        ExtJobOutcome::Failed { created_path: Some(ref path), .. } if *path == destination
    ));
    let mut cleanup = cleanup.expect("both cleanup failures stay owned");
    assert_eq!(cleanup.directories.len(), 2);
    assert!(cleanup.retry().is_err());
    for (index, (path, held)) in replaced.into_iter().enumerate() {
        assert_eq!(
            fs::read(path.join("other-data")).unwrap(),
            b"keep this replacement"
        );
        fs::rename(
            &path,
            directory.path().join(format!("kept-replacement-{index}")),
        )
        .unwrap();
        fs::rename(held, path).unwrap();
    }
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
