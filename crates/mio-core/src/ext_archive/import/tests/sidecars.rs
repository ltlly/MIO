use super::*;
use std::os::unix::fs::symlink;

#[test]
fn conversion_rejects_existing_and_late_sqlite_sidecars_without_changes() {
    let mut failures = Vec::new();
    for suffix in ["-wal", "-shm", "-journal"] {
        for dangling in [false, true] {
            for late in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let (source, bytes) = fixture(directory.path(), 72);
                let before = Identity::of(&fs::metadata(&source).unwrap()).unwrap();
                let destination = directory.path().join("target.mio");
                let sidecar = directory.path().join(format!("target.mio{suffix}"));
                let link_target = directory.path().join("absent-target");
                let create_sidecar = || {
                    if dangling {
                        symlink(&link_target, &sidecar).unwrap();
                    } else {
                        fs::write(&sidecar, b"user data").unwrap();
                    }
                    fs::symlink_metadata(&sidecar).unwrap()
                };
                let mut original = (!late).then(create_sidecar);
                let (outcome, cleanup) = convert(
                    inspected(source.clone()),
                    destination.clone(),
                    &CancellationToken::new(),
                    &mut |phase, completed, _| {
                        if late && phase == ExtImportPhase::Publish && completed == 0 {
                            original = Some(create_sidecar());
                        }
                    },
                );
                let original = original.unwrap();
                let after = fs::symlink_metadata(&sidecar).unwrap();
                let preserved = original.dev() == after.dev()
                    && original.ino() == after.ino()
                    && original.file_type() == after.file_type()
                    && if dangling {
                        fs::read_link(&sidecar).unwrap() == link_target
                    } else {
                        fs::read(&sidecar).unwrap() == b"user data"
                    };
                let rejected = matches!(
                    outcome,
                    ExtJobOutcome::Failed {
                        created_path: None,
                        ..
                    }
                );
                let published = fs::symlink_metadata(&destination).is_ok();
                if !rejected || published || !preserved {
                    failures.push(format!(
                        "{suffix}, dangling={dangling}, late={late}: \
                         rejected={rejected}, published={published}, preserved={preserved}"
                    ));
                }
                assert!(cleanup.is_none());
                assert!(fs::symlink_metadata(link_target).is_err());
                assert_eq!(fs::read(&source).unwrap(), bytes);
                assert_eq!(
                    Identity::of(&fs::metadata(&source).unwrap()).unwrap(),
                    before
                );
                assert!(
                    directory_names(directory.path())
                        .iter()
                        .all(|name| !name.starts_with(".mio-ext-"))
                );
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
