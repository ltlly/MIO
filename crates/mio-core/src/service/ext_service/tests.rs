use super::*;
use crate::ext_archive::import::tests::fixture;

mod cleanup_and_acks;
mod intruder_backpressure;
mod queries_and_items;

async fn finish(state: &mut State) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while state.ext_archive.active.is_some() {
            let signal = state.ext_archive.next_signal(&state.events, false).await;
            state.ext_signal(signal).await;
        }
    })
    .await
    .expect("bounded External worker cleanup");
}

fn outcomes(receiver: &mut mpsc::Receiver<Event>) -> Vec<(Uuid, ExtJobOutcome)> {
    let mut values = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        if let Event::ExtJob {
            job_id, outcome, ..
        } = event
        {
            values.push((job_id, outcome));
        }
    }
    values
}

#[tokio::test]
async fn inspection_release_is_acknowledged_after_snapshot_cleanup_and_cannot_be_reused() {
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = fixture(directory.path(), 72);
    let (events, mut receiver) = mpsc::channel(32);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let job = Uuid::new_v4();
    state
        .inspect_ext(state.project, job, path, CancellationToken::new())
        .await;
    finish(&mut state).await;
    let values = outcomes(&mut receiver);
    assert_eq!(values.len(), 1);
    let ExtJobOutcome::Inspected(info) = &values[0].1 else {
        panic!("{values:?}");
    };
    let inspection_id = info.id;
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    let release = Uuid::new_v4();
    state
        .release_ext(state.project, release, inspection_id)
        .await;
    assert!(outcomes(&mut receiver).is_empty());
    finish(&mut state).await;
    assert!(
        matches!(outcomes(&mut receiver).as_slice(), [(id, ExtJobOutcome::Released { inspection_id: released })] if *id == release && *released == inspection_id)
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    state
        .convert_ext(
            state.project,
            Uuid::new_v4(),
            inspection_id,
            directory.path().join("invalid.mio"),
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(
        outcomes(&mut receiver).as_slice(),
        [(_, ExtJobOutcome::Failed { .. })]
    ));
}

#[tokio::test]
async fn save_as_rejects_while_ext_inspection_is_retained() {
    let directory = tempfile::tempdir().unwrap();
    let (source, bytes) = fixture(directory.path(), 72);
    let inspection =
        import::inspect(source.clone(), &CancellationToken::new(), &mut |_, _, _| {}).unwrap();
    let inspection_id = inspection.info.id;
    let destination = directory.path().join("current.mio");
    let (events, mut receiver) = mpsc::channel(32);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let original_id = state.store.id;
    let workspace = state.store.workspace().unwrap();
    state.ext_archive.inspection = Some((state.project, inspection));
    let saved = state
        .command(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: state.project,
            path: Some(destination.clone()),
            workspace: None,
        })
        .await;
    let path_after_save = state.store.path.clone();
    let destination_created = destination.exists();
    let mut saved_ack = false;
    while let Ok(event) = receiver.try_recv() {
        saved_ack |= matches!(event, Event::ProjectSaved { .. });
    }

    // Reclaim test resources even when the unsafe save succeeds.
    let release = Uuid::new_v4();
    state
        .release_ext(state.project, release, inspection_id)
        .await;
    finish(&mut state).await;
    assert!(state.ext_archive.can_quiesce());
    let released = outcomes(&mut receiver);
    let current_id = state.store.id;
    let current_workspace = state.store.workspace();
    let closed = state.store.close();
    assert!(
        matches!(released.as_slice(), [(id, ExtJobOutcome::Released { inspection_id: actual })]
        if *id == release && *actual == inspection_id)
    );
    assert!(!destination.exists());
    assert_eq!(std::fs::read(source).unwrap(), bytes);
    assert!(
        saved.is_err() && path_after_save.is_none() && !destination_created && !saved_ack,
        "Save As while an inspection is retained must be rejected without switching or acknowledging success: saved={saved:?}, path={path_after_save:?}, destination_created={destination_created}, saved_ack={saved_ack}"
    );
    assert_eq!(current_id, original_id);
    assert_eq!(current_workspace.unwrap(), workspace);
    closed.unwrap();
}

#[tokio::test]
async fn explicit_new_project_rejects_owned_destination_before_publication() {
    let mut failures = Vec::new();
    for ext_owned in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let (events, _receiver) = mpsc::channel(32);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let project = state.project;
        let original = state.store.test_actual_path();
        let before = std::fs::read(&original).unwrap();
        let destination = if ext_owned {
            let (source, _) = fixture(directory.path(), 72);
            let inspection =
                import::inspect(source, &CancellationToken::new(), &mut |_, _, _| {}).unwrap();
            state.ext_archive.inspection = Some((project, inspection));
            directory.path().join("created.mio")
        } else {
            original.parent().unwrap().join("created.mio")
        };
        let result = state
            .command(Command::NewProject {
                path: Some(destination.clone()),
            })
            .await;
        let preserved = state.project == project
            && state.store.path.is_none()
            && std::fs::read(&original).is_ok_and(|bytes| bytes == before)
            && (!ext_owned || state.ext_archive.inspection.is_some());
        let destination_absent = !destination.exists();
        let cleaned = state.stop_ext().await.is_ok();
        let closed = state.store.close().is_ok();
        if result.is_ok() || !preserved || !destination_absent || !cleaned || !closed {
            failures.push(format!(
                "ext_owned={ext_owned}: result={result:?}, preserved={preserved}, \
                 destination_absent={destination_absent}, cleaned={cleaned}, closed={closed}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "explicit New must reject cleanup-owned destinations before creating or switching:\n{}",
        failures.join("\n")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn open_project_rejects_owned_sources_including_directory_and_file_aliases() {
    use std::os::unix::fs::symlink;

    let mut failures = Vec::new();
    for case in [
        "direct",
        "parent_alias",
        "file_alias",
        "outward_alias",
        "external_file_chain",
        "ext_archive",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (events, _receiver) = mpsc::channel(32);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let project = state.project;
        let original = state.store.test_actual_path();
        let original_before = std::fs::read(&original).unwrap();
        let source = match case {
            "direct" => original.clone(),
            "parent_alias" => {
                let link = directory.path().join("parent");
                symlink(original.parent().unwrap(), &link).unwrap();
                link.join(original.file_name().unwrap())
            }
            "file_alias" => {
                let link = directory.path().join("alias.mio");
                symlink(&original, &link).unwrap();
                link
            }
            "outward_alias" | "external_file_chain" => {
                let source = directory.path().join("external.mio");
                ProjectStore::create(Some(&source))
                    .unwrap()
                    .close()
                    .unwrap();
                let link = original.parent().unwrap().join("out");
                symlink(directory.path(), &link).unwrap();
                if case == "external_file_chain" {
                    let alias = directory.path().join("through-owned.mio");
                    symlink(link.join("external.mio"), &alias).unwrap();
                    alias
                } else {
                    link.join("external.mio")
                }
            }
            "ext_archive" => {
                let (source, _) = fixture(directory.path(), 72);
                let inspection =
                    import::inspect(source, &CancellationToken::new(), &mut |_, _, _| {}).unwrap();
                let source = directory.path().join("existing.mio");
                ProjectStore::create(Some(&source))
                    .unwrap()
                    .close()
                    .unwrap();
                state.ext_archive.inspection = Some((project, inspection));
                source
            }
            _ => unreachable!(),
        };
        let source_before = std::fs::read(&source).unwrap();
        let result = state
            .command(Command::OpenProject {
                path: source.clone(),
            })
            .await;
        let preserved = state.project == project
            && state.store.path.is_none()
            && std::fs::read(&original).is_ok_and(|bytes| bytes == original_before)
            && std::fs::read(&source).is_ok_and(|bytes| bytes == source_before)
            && (case != "ext_archive" || state.ext_archive.inspection.is_some());
        let cleaned = state.stop_ext().await.is_ok();
        let closed = state.store.close().is_ok();
        if result.is_ok() || !preserved || !cleaned || !closed {
            failures.push(format!(
                "{case}: result={result:?}, preserved={preserved}, cleaned={cleaned}, closed={closed}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "Open must reject cleanup-owned sources before changing either project:\n{}",
        failures.join("\n")
    );
}

#[tokio::test]
async fn conversion_rejects_destination_inside_current_temporary_project() {
    let directory = tempfile::tempdir().unwrap();
    let (source, bytes) = fixture(directory.path(), 72);
    let inspection =
        import::inspect(source.clone(), &CancellationToken::new(), &mut |_, _, _| {}).unwrap();
    let inspection_id = inspection.info.id;
    let (events, mut receiver) = mpsc::channel(32);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let project = state.project;
    let original = state.store.test_actual_path();
    let before = std::fs::read(&original).unwrap();
    let destination = original.parent().unwrap().join("converted.mio");
    state.ext_archive.inspection = Some((project, inspection));
    state
        .command(Command::ConvertExternal {
            project,
            job_id: Uuid::new_v4(),
            inspection_id,
            destination: destination.clone(),
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    finish(&mut state).await;
    let values = outcomes(&mut receiver);
    let destination_absent = !destination.exists();
    let preserved = state.project == project
        && state.store.path.is_none()
        && std::fs::read(&original).is_ok_and(|bytes| bytes == before);
    let clean = state.ext_archive.can_quiesce();
    let cleanup = state.stop_ext().await;
    let closed = state.store.close();
    assert_eq!(std::fs::read(source).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    assert!(
        matches!(
            values.as_slice(),
            [(
                _,
                ExtJobOutcome::Failed {
                    created_path: None,
                    ..
                }
            )]
        ) && destination_absent
            && preserved
            && clean,
        "conversion must reject the current project's temporary directory and release its inspection: outcomes={values:?}, destination_absent={destination_absent}, preserved={preserved}, clean={clean}"
    );
    cleanup.unwrap();
    closed.unwrap();
}

#[tokio::test]
async fn save_waits_for_active_ext_cleanup_and_succeeds_after_join() {
    let directory = tempfile::tempdir().unwrap();
    let (source, bytes) = fixture(directory.path(), 72);
    let inspection =
        import::inspect(source.clone(), &CancellationToken::new(), &mut |_, _, _| {}).unwrap();
    let inspection_id = inspection.info.id;
    let (events, mut receiver) = mpsc::channel(32);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let workspace = state.store.workspace().unwrap();
    let original = state.store.test_actual_path();
    let before = std::fs::read(&original).unwrap();
    let (started, ready) = oneshot::channel();
    let (resume, gate) = oneshot::channel::<()>();
    state.ext_archive.start(
        state.project,
        Uuid::new_v4(),
        CancellationToken::new(),
        true,
        move |_, _| {
            started.send(()).unwrap();
            let _ = gate.blocking_recv();
            inspection.release();
            Output::Finished(ExtJobOutcome::Released { inspection_id }, None)
        },
    );
    tokio::time::timeout(Duration::from_secs(1), ready)
        .await
        .unwrap()
        .unwrap();
    let destination = directory.path().join("after-join.mio");
    let blocked = state
        .command(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: state.project,
            path: Some(destination.clone()),
            workspace: None,
        })
        .await;
    let preserved = state.store.path.is_none()
        && !destination.exists()
        && std::fs::read(&original).is_ok_and(|bytes| bytes == before);
    let mut saved_ack = false;
    while let Ok(event) = receiver.try_recv() {
        saved_ack |= matches!(event, Event::ProjectSaved { .. });
    }
    resume.send(()).unwrap();
    finish(&mut state).await;
    let clean = state.ext_archive.can_quiesce();
    let saved = state
        .command(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: state.project,
            path: Some(destination.clone()),
            workspace: None,
        })
        .await;
    let closed = state.store.close();
    assert!(
        blocked.is_err() && preserved && !saved_ack && clean,
        "an active cleanup must reject Save without a success ACK: result={blocked:?}, preserved={preserved}, saved_ack={saved_ack}, clean={clean}"
    );
    saved.unwrap();
    closed.unwrap();
    let reopened = ProjectStore::open(&destination).unwrap();
    assert_eq!(reopened.workspace().unwrap(), workspace);
    reopened.close().unwrap();
    assert_eq!(std::fs::read(source).unwrap(), bytes);
}

#[tokio::test]
async fn cancelled_inspection_completion_is_released_before_cancelled_ack() {
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = fixture(directory.path(), 72);
    let (events, mut receiver) = mpsc::channel(32);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let cancel = CancellationToken::new();
    let job = Uuid::new_v4();
    let inspection = import::inspect(path, &cancel, &mut |_, _, _| {}).unwrap();
    let ready_cancel = cancel.clone();
    state
        .ext_archive
        .start(state.project, job, cancel.clone(), true, move |_, _| {
            ready_cancel.cancel();
            Output::Inspected(Box::new(inspection))
        });
    finish(&mut state).await;
    assert!(
        matches!(outcomes(&mut receiver).as_slice(), [(id, ExtJobOutcome::Cancelled)] if *id == job)
    );
    assert!(state.ext_archive.inspection.is_none());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}
