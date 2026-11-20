use super::*;

#[cfg(unix)]
#[test]
fn save_as_rejects_destination_inside_its_owned_temporary_directory() {
    use std::os::unix::fs::symlink;

    let mut failures = Vec::new();
    for alias in ["direct", "parent_alias", "outward_alias", "external_chain"] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ProjectStore::create(None).unwrap();
        let original_id = store.id;
        let record = record();
        store.put(&record).unwrap();
        let workspace = store.workspace().unwrap();
        store.checkpoint().unwrap();
        let source = store.actual_path();
        let source_before = fs::read(&source).unwrap();
        let owned = store.temporary.as_ref().unwrap().path();
        let destination = match alias {
            "direct" => owned.join("child.mio"),
            "parent_alias" => {
                let link = directory.path().join("parent-alias");
                symlink(owned, &link).unwrap();
                link.join("child.mio")
            }
            "outward_alias" | "external_chain" => {
                let link = owned.join("out");
                symlink(directory.path(), &link).unwrap();
                if alias == "external_chain" {
                    let alias = directory.path().join("through-owned");
                    symlink(&link, &alias).unwrap();
                    alias.join("child.mio")
                } else {
                    link.join("child.mio")
                }
            }
            _ => unreachable!(),
        };
        let error = store
            .save(Some(&destination))
            .err()
            .map(|error| format!("{error:#}"));
        let still_temporary =
            store.id == original_id && store.path.is_none() && store.temporary.is_some();
        let source_unchanged = fs::read(&source).is_ok_and(|bytes| bytes == source_before);
        let data_unchanged = store
            .read(record.summary.id)
            .is_ok_and(|stored| stored == record)
            && store.workspace().is_ok_and(|stored| stored == workspace);
        let destination_absent = !destination.exists();
        let external_absent = !directory.path().join("child.mio").exists();
        let closed = store.close().is_ok();
        if error.is_none()
            || !still_temporary
            || !source_unchanged
            || !data_unchanged
            || !destination_absent
            || !external_absent
            || !closed
        {
            failures.push(format!(
                "alias={alias}: result={}, still_temporary={still_temporary}, \
                 source_unchanged={source_unchanged}, data_unchanged={data_unchanged}, \
                 destination_absent={destination_absent}, external_absent={external_absent}, \
                 closed={closed}",
                error.as_deref().unwrap_or("Ok")
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "Save As must reject a destination deleted with its owned temporary source:\n{}",
        failures.join("\n")
    );
}

#[cfg(unix)]
#[test]
fn save_as_through_an_external_directory_alias_keeps_the_requested_path() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let real = directory.path().join("real");
    fs::create_dir(&real).unwrap();
    let alias = directory.path().join("alias");
    symlink(&real, &alias).unwrap();
    let destination = alias.join("saved.mio");
    let mut store = ProjectStore::create(None).unwrap();
    let record = record();
    store.put(&record).unwrap();
    assert_eq!(store.save(Some(&destination)).unwrap().0, destination);
    assert_eq!(store.path.as_ref(), Some(&destination));
    assert!(real.join("saved.mio").is_file());
    store.save(None).unwrap();
    store.close().unwrap();
    let reopened = ProjectStore::open(&destination).unwrap();
    assert_eq!(reopened.read(record.summary.id).unwrap(), record);
    reopened.close().unwrap();
}

#[cfg(unix)]
fn existing_sidecars_remain_untouched(save_as: bool) {
    use std::os::unix::fs::{MetadataExt, symlink};

    let mut failures = Vec::new();
    for suffix in ["-wal", "-shm", "-journal"] {
        for dangling in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let destination = directory.path().join("target.mio");
            let sidecar = directory.path().join(format!("target.mio{suffix}"));
            let link_target = directory.path().join("must-stay-absent");
            let source = directory.path().join("source.mio");
            let mut store = save_as.then(|| {
                let mut store = ProjectStore::create(Some(&source)).unwrap();
                store.put(&record()).unwrap();
                store.checkpoint().unwrap();
                store
            });
            let source_before = save_as.then(|| fs::read(&source).unwrap());
            if dangling {
                symlink(&link_target, &sidecar).unwrap();
            } else {
                fs::write(&sidecar, b"sidecar!").unwrap();
            }
            let before = fs::symlink_metadata(&sidecar).unwrap();
            let result = if let Some(store) = &mut store {
                store.save(Some(&destination)).map(|_| ())
            } else {
                ProjectStore::create(Some(&destination)).map(drop)
            };
            drop(store);
            let error = result.err().map(|error| format!("{error:#}"));
            let published = fs::symlink_metadata(&destination).is_ok();
            let same_identity = fs::symlink_metadata(&sidecar).is_ok_and(|after| {
                (before.dev(), before.ino(), before.file_type())
                    == (after.dev(), after.ino(), after.file_type())
            });
            let same_content = if dangling {
                fs::read_link(&sidecar).is_ok_and(|target| target == link_target)
            } else {
                fs::read(&sidecar).is_ok_and(|bytes| bytes == b"sidecar!")
            };
            let target_absent = fs::symlink_metadata(&link_target)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
            let source_unchanged = source_before
                .is_none_or(|before| fs::read(&source).is_ok_and(|after| after == before));
            if error.is_none()
                || published
                || !same_identity
                || !same_content
                || !target_absent
                || !source_unchanged
            {
                failures.push(format!(
                    "{suffix}, dangling={dangling}: result={}, published={published}, \
                     same_identity={same_identity}, same_content={same_content}, \
                     target_absent={target_absent}, source_unchanged={source_unchanged}",
                    error.as_deref().unwrap_or("Ok")
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "new project destinations must reject existing SQLite sidecars without changing them:\n{}",
        failures.join("\n")
    );
}

#[cfg(unix)]
#[test]
fn create_rejects_existing_and_dangling_sqlite_sidecars_without_changes() {
    existing_sidecars_remain_untouched(false);
}

#[cfg(unix)]
#[test]
fn save_as_rejects_existing_and_dangling_sqlite_sidecars_without_changes() {
    existing_sidecars_remain_untouched(true);
}

#[cfg(unix)]
#[test]
fn new_project_destinations_preserve_dangling_main_symlinks() {
    use std::os::unix::fs::{MetadataExt, symlink};

    for save_as in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("target.mio");
        let link_target = directory.path().join("must-stay-absent");
        symlink(&link_target, &destination).unwrap();
        let before = fs::symlink_metadata(&destination).unwrap();
        let result = if save_as {
            let mut store = ProjectStore::create(None).unwrap();
            let result = store.save(Some(&destination)).map(|_| ());
            assert!(store.path.is_none());
            result
        } else {
            ProjectStore::create(Some(&destination)).map(drop)
        };
        assert!(result.is_err());
        let after = fs::symlink_metadata(&destination).unwrap();
        assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
        assert!(after.file_type().is_symlink());
        assert_eq!(fs::read_link(&destination).unwrap(), link_target);
        assert!(
            fs::symlink_metadata(&link_target)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        );
    }
}

#[test]
fn ordinary_same_path_save_allows_its_own_live_wal_and_shm() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("current.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let record = record();
    store.put(&record).unwrap();
    for suffix in ["-wal", "-shm"] {
        assert!(
            directory
                .path()
                .join(format!("current.mio{suffix}"))
                .exists()
        );
    }
    assert_eq!(store.save(Some(&path)).unwrap().0, path);
    assert_eq!(store.read(record.summary.id).unwrap(), record);
}

#[cfg(unix)]
#[test]
fn new_project_path_inspection_errors_fail_closed_without_publication() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().join("unsearchable");
    fs::create_dir(&parent).unwrap();
    let destination = parent.join("target.mio");
    let mut store = ProjectStore::create(None).unwrap();
    let record = record();
    store.put(&record).unwrap();
    let original = fs::metadata(&parent).unwrap().permissions();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o000)).unwrap();
    let checked = check_new_project_path(&destination);
    let created = ProjectStore::create(Some(&destination)).map(drop);
    let saved = store.save(Some(&destination)).map(|_| ());
    fs::set_permissions(&parent, original).unwrap();
    for result in [checked, created, saved] {
        let error = result.expect_err("permission errors are not evidence of a free path");
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
    assert!(!destination.exists());
    assert!(store.path.is_none());
    assert_eq!(store.read(record.summary.id).unwrap(), record);
}
