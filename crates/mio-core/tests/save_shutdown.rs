#![cfg(unix)]

use std::{
    env,
    ffi::{CStr, OsStr, c_char, c_int},
    fs,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, ensure};
use mio_core::{Command, Event, Runtime};
use rusqlite::ffi;
use tokio::{process::Command as ProcessCommand, time::timeout};

const CHILD_ROOT: &str = "MIO_SAVE_SHUTDOWN_CHILD_ROOT";
const TEST_NAME: &str = "cancelled_save_as_never_publishes_or_leaves_staging";
const STEP_BUDGET: Duration = Duration::from_secs(3);
const CHILD_BUDGET: Duration = Duration::from_secs(15);

type OpenDirectory = unsafe extern "C" fn(*const c_char, *mut c_int) -> c_int;
type SetSystemCall =
    unsafe extern "C" fn(*mut ffi::sqlite3_vfs, *const c_char, ffi::sqlite3_syscall_ptr) -> c_int;

struct DirectoryGate {
    directory: PathBuf,
    original: OpenDirectory,
    entered: mpsc::SyncSender<PathBuf>,
    release: Mutex<mpsc::Receiver<()>>,
    intercepted: AtomicBool,
    handshake_failed: AtomicBool,
}

static GATE: OnceLock<DirectoryGate> = OnceLock::new();

unsafe extern "C" fn gated_open_directory(filename: *const c_char, fd: *mut c_int) -> c_int {
    let Some(gate) = GATE.get() else {
        if !fd.is_null() {
            unsafe { *fd = -1 };
        }
        return ffi::SQLITE_CANTOPEN;
    };
    // SQLite owns this NUL-terminated filename for the duration of the callback.
    let path = Path::new(OsStr::from_bytes(
        unsafe { CStr::from_ptr(filename) }.to_bytes(),
    ));
    if path.parent() == Some(gate.directory.as_path())
        && path.file_name().is_some_and(|name| {
            name.as_bytes().starts_with(b".tmp") && name.as_bytes().ends_with(b"-journal")
        })
        && !gate.intercepted.swap(true, Ordering::SeqCst)
    {
        let announced = gate.entered.try_send(path.to_path_buf()).is_ok();
        let released = announced
            && gate
                .release
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .recv_timeout(STEP_BUDGET)
                .is_ok();
        gate.handshake_failed.store(!released, Ordering::SeqCst);
    }
    unsafe { (gate.original)(filename, fd) }
}

struct VfsHook {
    vfs: *mut ffi::sqlite3_vfs,
    set: SetSystemCall,
    original: ffi::sqlite3_syscall_ptr,
}

impl VfsHook {
    fn install(
        directory: PathBuf,
        entered: mpsc::SyncSender<PathBuf>,
        release: mpsc::Receiver<()>,
    ) -> Result<Self> {
        // Unix syscall overrides are process-wide; install before starting this child's Runtime.
        let vfs = unsafe { ffi::sqlite3_vfs_find(std::ptr::null()) };
        ensure!(!vfs.is_null(), "SQLite default VFS is unavailable");
        let methods = unsafe { &*vfs };
        ensure!(
            methods.iVersion >= 3,
            "SQLite VFS has no syscall override API"
        );
        let get = methods.xGetSystemCall.context("missing xGetSystemCall")?;
        let set = methods.xSetSystemCall.context("missing xSetSystemCall")?;
        let original = unsafe { get(vfs, c"openDirectory".as_ptr()) }
            .context("SQLite openDirectory syscall is unavailable")?;
        // The pinned Unix VFS declares openDirectory as int(const char *, int *).
        let open_directory =
            unsafe { std::mem::transmute::<unsafe extern "C" fn(), OpenDirectory>(original) };
        GATE.set(DirectoryGate {
            directory,
            original: open_directory,
            entered,
            release: Mutex::new(release),
            intercepted: AtomicBool::new(false),
            handshake_failed: AtomicBool::new(false),
        })
        .map_err(|_| anyhow!("directory gate was already installed"))?;
        let replacement = unsafe {
            std::mem::transmute::<OpenDirectory, unsafe extern "C" fn()>(gated_open_directory)
        };
        let rc = unsafe { set(vfs, c"openDirectory".as_ptr(), Some(replacement)) };
        ensure!(rc == ffi::SQLITE_OK, "xSetSystemCall failed: {rc}");
        Ok(Self {
            vfs,
            set,
            original: Some(original),
        })
    }

    fn restore(&mut self) -> Result<()> {
        if self.original.is_some() {
            let rc = unsafe { (self.set)(self.vfs, c"openDirectory".as_ptr(), self.original) };
            ensure!(rc == ffi::SQLITE_OK, "restoring openDirectory failed: {rc}");
            self.original = None;
        }
        Ok(())
    }
}

impl Drop for VfsHook {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn entries(directory: &Path) -> std::io::Result<Vec<PathBuf>> {
    fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect()
}

async fn exercise_child(root: PathBuf) -> Result<()> {
    let started = Instant::now();
    let runtime_directory = root.join("runtime");
    let destination_directory = root.join("destination");
    let destination = destination_directory.join("discarded.mio");
    ensure!(
        env::temp_dir().canonicalize()? == runtime_directory,
        "Runtime temporary files must stay inside the child fixture"
    );
    ensure!(entries(&runtime_directory)?.is_empty());
    ensure!(entries(&destination_directory)?.is_empty());
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let mut hook = VfsHook::install(destination_directory.clone(), entered_tx, release_rx)?;
    let (runtime, mut client) = Runtime::start()?;
    let observation: Result<(PathBuf, PathBuf)> = async {
        let opened = timeout(STEP_BUDGET, client.events.recv())
            .await
            .context("ProjectOpened timed out")?
            .context("Runtime disconnected before ProjectOpened")?;
        let Event::ProjectOpened {
            project,
            path: None,
            ..
        } = opened
        else {
            return Err(anyhow!("expected a real temporary project, got {opened:?}"));
        };
        let temporary_projects = entries(&runtime_directory)?;
        ensure!(
            temporary_projects.len() == 1,
            "expected one owned temporary project"
        );
        ensure!(temporary_projects[0].join("project.mio").is_file());
        client.commands.try_send(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project,
            path: Some(destination.clone()),
            workspace: None,
        })?;
        let journal = entered_rx
            .recv_timeout(STEP_BUDGET)
            .context("SaveProject did not reach its staging directory open")?;
        let filename = journal
            .file_name()
            .context("staging journal has no filename")?;
        let basename = filename
            .as_bytes()
            .strip_suffix(b"-journal")
            .context("intercepted file is not a staging journal")?;
        let staging = journal.with_file_name(OsStr::from_bytes(basename));
        ensure!(
            staging.is_file() && journal.is_file(),
            "staging files must already exist"
        );
        ensure!(
            !destination.try_exists()?,
            "target was published before the gate"
        );
        Ok((staging, journal))
    }
    .await;

    client.shutdown.cancel();
    let released = release_tx.try_send(());
    let join_started = Instant::now();
    // Await the actual join; the parent watchdog must not turn a detached join into success.
    let joined = tokio::task::spawn_blocking(move || runtime.shutdown()).await;
    let join_elapsed = join_started.elapsed();
    drop(client);
    let restored = hook.restore();
    let remaining_destination = entries(&destination_directory);
    let remaining_runtime = entries(&runtime_directory);
    let target_exists = destination.try_exists();

    restored?;
    joined.context("Runtime shutdown worker panicked")??;
    released.context("could not explicitly release the directory gate")?;
    let (staging, journal) = observation?;
    let gate = GATE.get().context("directory gate disappeared")?;
    ensure!(
        gate.intercepted.load(Ordering::SeqCst) && !gate.handshake_failed.load(Ordering::SeqCst),
        "the gate must observe an explicit release, not a timeout or disconnect"
    );
    let remaining_destination = remaining_destination?;
    let remaining_runtime = remaining_runtime?;
    let target_exists = target_exists?;
    let staging_reclaimed = !staging.try_exists()? && !journal.try_exists()?;
    eprintln!(
        "save_shutdown: join_ms={} child_ms={} target_exists={target_exists} \
         staging_reclaimed={staging_reclaimed} runtime_entries={} destination_entries={}",
        join_elapsed.as_millis(),
        started.elapsed().as_millis(),
        remaining_runtime.len(),
        remaining_destination.len(),
    );
    ensure!(
        join_elapsed <= STEP_BUDGET,
        "Runtime join exceeded {STEP_BUDGET:?}"
    );
    ensure!(
        !target_exists
            && staging_reclaimed
            && remaining_destination.is_empty()
            && remaining_runtime.is_empty(),
        "cancellation before publication must leave no target or owned artifacts: \
         target_exists={target_exists}, staging_reclaimed={staging_reclaimed}, \
         destination={remaining_destination:?}, runtime={remaining_runtime:?}"
    );
    Ok(())
}

#[tokio::test]
async fn cancelled_save_as_never_publishes_or_leaves_staging() -> Result<()> {
    if let Some(root) = env::var_os(CHILD_ROOT) {
        return exercise_child(PathBuf::from(root)).await;
    }
    let started = Instant::now();
    let directory = tempfile::Builder::new()
        .prefix("mio-save-shutdown-")
        .tempdir()?;
    let root = directory.path().canonicalize()?;
    fs::create_dir(root.join("runtime"))?;
    fs::create_dir(root.join("destination"))?;
    let log_path = root.join("child.log");
    let log = fs::File::create(&log_path)?;
    let mut child = ProcessCommand::new(env::current_exe()?)
        .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=1"])
        .env(CHILD_ROOT, &root)
        .env("TMPDIR", root.join("runtime"))
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .kill_on_drop(true)
        .spawn()?;
    let status = match timeout(CHILD_BUDGET, child.wait()).await {
        Ok(Ok(status)) => Ok(status),
        failure => {
            let kill = child.start_kill();
            let reaped = timeout(STEP_BUDGET, child.wait()).await;
            if !matches!(&reaped, Ok(Ok(_))) {
                let retained = directory.keep();
                return Err(anyhow!(
                    "fixture child was not reaped; retained {retained:?}; \
                     wait={failure:?}, kill={kill:?}, reap={reaped:?}"
                ));
            }
            Err(anyhow!(
                "fixture child did not finish normally: {failure:?}; kill={kill:?}"
            ))
        }
    };
    let output = fs::read_to_string(&log_path);
    let cleaned = directory.close();
    eprintln!(
        "save_shutdown: total_wall_ms={}",
        started.elapsed().as_millis()
    );
    cleaned.context("cleaning the isolated fixture failed")?;
    let output = output.context("reading the child result failed")?;
    let status = status.with_context(|| output.clone())?;
    ensure!(
        status.success(),
        "isolated save/shutdown regression failed ({status}):\n{output}"
    );
    eprintln!("{output}");
    Ok(())
}
