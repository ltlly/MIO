use super::*;
use std::{fs, io};
#[cfg(unix)]
use std::{
    future::{Future, poll_fn},
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll, Wake, Waker},
};
#[cfg(unix)]
use tempfile::NamedTempFile;

#[cfg(unix)]
struct ReplaceDirectoryOnWake {
    owned: PathBuf,
    held: PathBuf,
    replacement: NamedTempFile,
    replaced: Mutex<Option<io::Result<()>>>,
}

#[cfg(unix)]
impl ReplaceDirectoryOnWake {
    fn reclaim(&self) -> io::Result<()> {
        let metadata = match fs::symlink_metadata(&self.owned) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let replacement = self.replacement.as_file().metadata()?;
        if !metadata.is_file()
            || (metadata.dev(), metadata.ino()) != (replacement.dev(), replacement.ino())
        {
            return Err(io::Error::other("refusing to remove a non-fixture path"));
        }
        fs::remove_file(&self.owned)
    }
}

#[cfg(unix)]
impl Wake for ReplaceDirectoryOnWake {
    fn wake(self: Arc<Self>) {
        let mut replaced = self
            .replaced
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if replaced.is_none() {
            *replaced = Some(
                fs::rename(&self.owned, &self.held)
                    .and_then(|()| fs::hard_link(self.replacement.path(), &self.owned)),
            );
        }
    }
}

#[cfg(unix)]
impl Drop for ReplaceDirectoryOnWake {
    fn drop(&mut self) {
        let _ = self.reclaim();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn final_temporary_store_cleanup_failure_reaches_state_run_result() -> Result<()> {
    let directory = tempfile::Builder::new()
        .prefix("mio-final-close-")
        .tempdir()?;
    let (events, mut received) = mpsc::channel(4);
    let shutdown = CancellationToken::new();
    let state = State::new(events, shutdown.clone())?;
    ensure!(
        state.store.path.is_none(),
        "fixture must own a temporary project"
    );
    let actual = state.store.test_actual_path();
    let owned = actual
        .parent()
        .context("temporary project has no directory")?
        .to_owned();
    let gate = Arc::new(ReplaceDirectoryOnWake {
        owned,
        held: directory.path().join("held-project"),
        replacement: NamedTempFile::new_in(directory.path())?,
        replaced: Mutex::new(None),
    });
    let (_commands, incoming) = mpsc::channel(1);
    let mut run = Box::pin(state.run(incoming));
    let startup = poll_fn(|cx| Poll::Ready(run.as_mut().poll(cx))).await;
    ensure!(
        startup.is_pending(),
        "State::run ended before shutdown: {startup:?}"
    );
    ensure!(
        matches!(
            received.try_recv()?,
            Event::ProjectOpened { path: None, .. }
        ),
        "the real temporary project must open before shutdown"
    );
    ensure!(
        matches!(
            received.try_recv()?,
            Event::ProxyState { address: None, .. }
        ),
        "the fixture must have no active Proxy"
    );

    let waker = Waker::from(gate.clone());
    let mut context = TaskContext::from_waker(&waker);
    ensure!(
        received.poll_recv(&mut context).is_pending(),
        "startup notifications must be drained before arming the gate"
    );
    // The idle state's next send is Stopped; its synchronous wake follows the final checkpoint.
    shutdown.cancel();
    let (completed, within_budget) =
        match tokio::time::timeout(Duration::from_secs(1), run.as_mut()).await {
            Ok(result) => (result, true),
            Err(_) => (run.as_mut().await, false),
        };
    drop(run);
    let stopped = received.try_recv();
    let disconnected = received.try_recv();
    drop(received);
    drop(waker);

    let replaced = gate
        .replaced
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let held_project = gate.held.join("project.mio").is_file();
    let replacement_present = fs::symlink_metadata(&gate.owned).map(|metadata| metadata.is_file());
    let reclaimed = gate.reclaim();
    drop(gate);
    let cleaned = directory.close();

    // Reclaim the replacement and moved project before any regression assertion can fail.
    reclaimed.context("reclaiming the fixture replacement failed")?;
    cleaned.context("reclaiming the moved project failed")?;
    replaced
        .context("the final notification never woke its receiver")?
        .context("replacing the owned directory failed")?;
    ensure!(
        matches!(stopped, Ok(Event::Stopped)),
        "the gate must follow the successful final checkpoint: {stopped:?}"
    );
    ensure!(
        matches!(disconnected, Err(mpsc::error::TryRecvError::Disconnected)),
        "no event sender may outlive State::run: {disconnected:?}"
    );
    ensure!(
        held_project,
        "the moved project must survive the failed cleanup"
    );
    ensure!(
        replacement_present?,
        "the failing directory path must remain a regular file"
    );
    ensure!(
        within_budget,
        "the idle State::run exceeded its shutdown budget"
    );
    let error = completed
        .expect_err("State::run must report final temporary-directory cleanup failure, not Ok");
    ensure!(
        error
            .chain()
            .any(|cause| cause.to_string() == "清理临时项目目录失败"),
        "shutdown must preserve the final directory-cleanup error: {error:#}"
    );
    ensure!(
        error
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::NotADirectory),
        "the regular-file replacement must cause NotADirectory: {error:#}"
    );
    Ok(())
}

#[tokio::test]
async fn successful_final_store_close_reclaims_only_temporary_files() -> Result<()> {
    let directory = tempfile::Builder::new()
        .prefix("mio-final-close-success-")
        .tempdir()?;
    let saved = directory.path().join("saved.mio");
    let checked = async {
        for named in [false, true] {
            let (events, mut received) = mpsc::channel(4);
            let mut state = State::new(events, CancellationToken::new())?;
            let actual = state.store.test_actual_path();
            let temporary = actual
                .parent()
                .context("temporary project has no directory")?
                .to_owned();
            let project_id = state.store.id;
            let record = tests::completed_flow();
            let mut workspace = state.store.workspace()?;
            workspace.open_request(Some(&record.request))?;
            state.store.save_workspace(&workspace)?;
            if named {
                state.store.save(Some(&saved))?;
            }
            state.store.put(&record)?;
            let (commands, incoming) = mpsc::channel(1);
            drop(commands);
            let mut run = Box::pin(state.run(incoming));
            let (completed, within_budget) =
                match tokio::time::timeout(Duration::from_secs(1), run.as_mut()).await {
                    Ok(result) => (result, true),
                    Err(_) => (run.as_mut().await, false),
                };
            drop(run);
            let mut stopped = 0;
            while let Ok(event) = received.try_recv() {
                if matches!(event, Event::Stopped) {
                    stopped += 1;
                }
            }
            drop(received);
            let temporary_removed = !temporary.try_exists()?;
            if !temporary_removed {
                fs::remove_dir_all(&temporary)?;
            }
            completed?;
            ensure!(
                within_budget,
                "final store close exceeded its shutdown budget"
            );
            ensure!(stopped == 1, "shutdown must emit exactly one Stopped event");
            ensure!(
                temporary_removed,
                "shutdown left its owned temporary project behind"
            );
            if named {
                let entries = fs::read_dir(directory.path())?
                    .map(|entry| entry.map(|entry| entry.path()))
                    .collect::<io::Result<Vec<_>>>()?;
                ensure!(
                    entries.as_slice() == std::slice::from_ref(&saved),
                    "the named project must survive without SQLite sidecars: {entries:?}"
                );
                let reopened = ProjectStore::open(&saved)?;
                let saved_id = reopened.id;
                let saved_workspace = reopened.workspace();
                let saved_record = reopened.read(record.summary.id);
                reopened.close()?;
                ensure!(
                    saved_id == project_id,
                    "closing changed the project identity"
                );
                ensure!(
                    saved_workspace? == workspace,
                    "closing lost the saved workspace"
                );
                ensure!(
                    saved_record? == record,
                    "closing lost the final binary flow write"
                );
            } else {
                ensure!(
                    !saved.try_exists()?,
                    "temporary shutdown published a named project"
                );
            }
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    directory
        .close()
        .context("cleaning the successful-close fixture failed")?;
    checked
}
