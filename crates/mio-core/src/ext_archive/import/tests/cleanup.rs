use super::*;

#[test]
fn changed_or_replaced_source_rejects_conversion_without_publishing() {
    for replace in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let (path, bytes) = fixture(directory.path(), 72);
        let inspection = inspected(path.clone());
        if replace {
            fs::rename(&path, directory.path().join("kept.archive")).unwrap();
            fs::write(&path, bytes).unwrap();
        } else {
            use std::io::Write;
            OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(&[1])
                .unwrap();
        }
        let current = fs::read(&path).unwrap();
        let destination = directory.path().join("rejected.mio");
        let (outcome, cleanup) = convert(
            inspection,
            destination.clone(),
            &CancellationToken::new(),
            &mut |_, _, _| {},
        );
        assert!(cleanup.is_none());
        assert!(matches!(
            outcome,
            ExtJobOutcome::Failed {
                created_path: None,
                ..
            }
        ));
        assert!(!destination.exists());
        assert_eq!(fs::read(&path).unwrap(), current);
        assert!(
            directory_names(directory.path())
                .iter()
                .all(|name| !name.starts_with(".mio-ext-"))
        );
    }
}

#[test]
fn destination_created_during_conversion_is_never_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = fixture(directory.path(), 72);
    let destination = directory.path().join("existing.mio");
    let (outcome, cleanup) = convert(
        inspected(path),
        destination.clone(),
        &CancellationToken::new(),
        &mut |phase, completed, _| {
            if phase == ExtImportPhase::Publish && completed == 0 {
                fs::write(&destination, b"user data").unwrap();
            }
        },
    );
    assert!(cleanup.is_none());
    assert!(matches!(
        outcome,
        ExtJobOutcome::Failed {
            created_path: None,
            ..
        }
    ));
    assert_eq!(fs::read(destination).unwrap(), b"user data");
    assert_eq!(
        directory_names(directory.path()),
        vec!["existing.mio", "synthetic.archive"]
    );
}
