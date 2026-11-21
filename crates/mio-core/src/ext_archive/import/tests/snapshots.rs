use super::*;

#[test]
fn conversion_persists_report_and_survives_save_save_as_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let (source, bytes) = fixture(directory.path(), 4096);
    let before = Identity::of(&fs::metadata(&source).unwrap()).unwrap();
    let destination = directory.path().join("converted.mio");
    let mut phases = Vec::new();
    let (outcome, cleanup) = convert(
        inspected(source.clone()),
        destination.clone(),
        &CancellationToken::new(),
        &mut |phase, completed, total| phases.push((phase, completed, total)),
    );
    assert!(cleanup.is_none());
    let ExtJobOutcome::Imported {
        report,
        total_bytes,
        ..
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    assert!(report.complete_archive);
    assert_eq!(report.source_bytes, bytes.len() as u64);
    assert_eq!(fs::metadata(&destination).unwrap().len(), total_bytes);
    assert_eq!(phases.last(), Some(&(ExtImportPhase::Publish, 1, Some(1))));
    assert_eq!(
        Identity::of(&fs::metadata(&source).unwrap()).unwrap(),
        before
    );
    assert_eq!(fs::read(&source).unwrap(), bytes);
    let mut project = ProjectStore::open(&destination).unwrap();
    let sources = project.ext_sources().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].report, report);
    project.save(None).unwrap();
    let saved = directory.path().join("saved.mio");
    project.save(Some(&saved)).unwrap();
    project.close().unwrap();
    let reopened = ProjectStore::open(&saved).unwrap();
    assert_eq!(reopened.ext_sources().unwrap(), sources);
    reopened.close().unwrap();
    assert!(
        directory_names(directory.path())
            .iter()
            .all(|name| !name.starts_with(".mio-ext-"))
    );
}

#[test]
fn multi_schema_compatibility_and_special_files_do_not_leave_inspection_resources() {
    let directory = tempfile::tempdir().unwrap();
    let (path, mut bytes) = fixture(directory.path(), 72);
    bytes[12..16].copy_from_slice(&[0, 226, 0, 226]);
    fs::write(&path, &bytes).unwrap();
    let inspection = inspect(path.clone(), &CancellationToken::new(), &mut |_, _, _| {}).unwrap();
    assert_eq!(
        (inspection.info.report.schema_floor, inspection.info.report.schema_revision),
        (226, 226)
    );
    inspection.release();
    #[cfg(unix)]
    assert!(
        inspect(
            PathBuf::from("/dev/null"),
            &CancellationToken::new(),
            &mut |_, _, _| {}
        )
        .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(directory_names(directory.path()), vec!["synthetic.archive"]);
}
