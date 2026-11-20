#[cfg(unix)]
pub(crate) use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
pub(crate) use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant},
};

pub(crate) use anyhow::{Context, Result, bail, ensure};
pub(crate) use mio_core::{
    ExtJobOutcome, ExtSourceInfo, Client, Command, Event, HttpHistoryFilter, HttpHistoryQuery,
    HttpHistorySort, ProjectKey, Runtime, Uuid, Workspace,
};
pub(crate) use tokio_util::sync::CancellationToken;

pub(crate) const USAGE: &str =
    "usage: ext_archive_project_check <source.archive> <new.mio> <save-as-new.mio>";
pub(crate) const WORK_BUDGET: Duration = Duration::from_secs(9 * 60 + 30);
pub(crate) const TOTAL_BUDGET: Duration = Duration::from_secs(10 * 60);
pub(crate) const PAGE: usize = 200;

#[derive(Debug)]
pub(crate) struct Paths {
    pub(crate) source: PathBuf,
    pub(crate) converted: PathBuf,
    pub(crate) saved: PathBuf,
}

#[derive(PartialEq, Eq)]
pub(crate) struct SourceStamp {
    #[cfg(unix)]
    pub(crate) device: u64,
    #[cfg(unix)]
    pub(crate) inode: u64,
    pub(crate) bytes: u64,
    #[cfg(unix)]
    pub(crate) modified: (i64, i64),
    #[cfg(unix)]
    pub(crate) changed: (i64, i64),
    #[cfg(not(unix))]
    pub(crate) modified_sys: Option<std::time::SystemTime>,
    #[cfg(not(unix))]
    pub(crate) created_sys: Option<std::time::SystemTime>,
}

impl SourceStamp {
    pub(crate) fn read(metadata: fs::Metadata) -> Result<Self> {
        ensure!(
            metadata.is_file(),
            "source must be a regular file, not a symbolic link or device"
        );
        #[cfg(unix)]
        {
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                bytes: metadata.len(),
                modified: (metadata.mtime(), metadata.mtime_nsec()),
                changed: (metadata.ctime(), metadata.ctime_nsec()),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                bytes: metadata.len(),
                modified_sys: metadata.modified().ok(),
                created_sys: metadata.created().ok(),
            })
        }
    }
}

pub(crate) struct Source {
    pub(crate) file: File,
    pub(crate) initial: SourceStamp,
}

impl Source {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        // O_NONBLOCK prevents a replacement FIFO from hanging before fstat can reject it.
        #[cfg(not(unix))]
        if let Ok(meta) = fs::symlink_metadata(path) {
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "cannot safely open the read-only source"
            );
        }
        let mut opts = OpenOptions::new();
        opts.read(true);
        #[cfg(unix)]
        opts.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
        let file = opts
            .open(path)
            .context("cannot safely open the read-only source")?;
        let initial = SourceStamp::read(file.metadata()?)?;
        let source = Self { file, initial };
        source.unchanged(path)?;
        Ok(source)
    }

    pub(crate) fn unchanged(&self, path: &Path) -> Result<()> {
        ensure!(
            SourceStamp::read(self.file.metadata()?)? == self.initial
                && SourceStamp::read(fs::symlink_metadata(path)?)? == self.initial,
            "source identity or metadata changed during verification"
        );
        Ok(())
    }
}

#[derive(PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) source: ExtSourceInfo,
    pub(crate) proxy_flows: usize,
    pub(crate) repeater_tabs: usize,
    pub(crate) repeater_groups: usize,
}
