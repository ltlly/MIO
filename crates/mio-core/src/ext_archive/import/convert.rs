use super::*;
use crate::{
    ExtImportPhase, ExtImportReport, ExtJobOutcome, Uuid,
    ext_archive::direct_import,
    store::{ProjectStore, check_new_project_path, check_outside_owned_directory},
};

pub(crate) fn inspect(
    path: PathBuf,
    cancel: &CancellationToken,
    progress: Progress<'_>,
) -> Result<Inspection> {
    check_cancel(cancel)?;
    #[cfg(not(unix))]
    if let Ok(meta) = fs::symlink_metadata(&path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "不能只读打开 External 源文件"
        );
    }
    let mut open_opts = OpenOptions::new();
    open_opts.read(true);
    #[cfg(unix)]
    open_opts.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        open_opts.share_mode(0x0000_0007);
    }
    #[allow(unused_mut)]
    let source = open_opts.open(&path).context("不能只读打开 External 源文件")?;
    let identity = Identity::of(&source.metadata()?)?;
    #[cfg(windows)]
    let win_file_id = super::windows_handle_file_id(&source)?;
    ensure!(
        Identity::of(&fs::symlink_metadata(&path)?)? == identity,
        "External 源路径在打开时已变化"
    );
    #[cfg(windows)]
    ensure!(
        super::windows_path_file_id(&path).ok() == Some(win_file_id),
        "External 源路径在打开时已变化"
    );
    progress(ExtImportPhase::Snapshot, 0, Some(identity.length));
    let result = (|| -> Result<ExtImportReport> {
        let report = direct_import::inspect_report(&path, cancel, progress)?;
        ensure!(
            Identity::of(&source.metadata()?)? == identity
                && Identity::of(&fs::symlink_metadata(&path)?)? == identity,
            "External 源文件在预检期间发生变化，请重试预检"
        );
        #[cfg(windows)]
        ensure!(
            super::windows_handle_file_id(&source).ok() == Some(win_file_id)
                && super::windows_path_file_id(&path).ok() == Some(win_file_id),
            "External 源文件在预检期间发生变化，请重试预检"
        );
        check_cancel(cancel)?;
        Ok(report)
    })();
    let report = match result {
        Ok(report) => report,
        Err(error) => {
            drop(source);
            return Err(error);
        }
    };
    Ok(Inspection {
        info: ExtInspection {
            id: Uuid::new_v4(),
            source_name: path
                .file_name()
                .context("External 源文件名无效")?
                .to_string_lossy()
                .into_owned(),
            report,
        },
        source_path: path,
        source,
        identity,
        #[cfg(windows)]
        win_file_id,
        snapshot: None,
    })
}

pub(crate) fn failed(error: anyhow::Error, created_path: Option<PathBuf>) -> ExtJobOutcome {
    if created_path.is_none() && error.is::<Cancelled>() {
        ExtJobOutcome::Cancelled
    } else {
        ExtJobOutcome::Failed {
            message: format!("{error:#}"),
            created_path,
        }
    }
}

pub(crate) fn convert(
    inspection: Inspection,
    destination: PathBuf,
    cancel: &CancellationToken,
    progress: Progress<'_>,
) -> (ExtJobOutcome, Option<CleanupFailure>) {
    let mut stage = None;
    let mut published = None;
    let result = (|| -> Result<(ExtImportReport, u64)> {
        check_cancel(cancel)?;
        inspection.check_source()?;
        ensure!(
            destination
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("mio")),
            "外部归档转换目标必须是新的 .mio 文件"
        );
        check_new_project_path(&destination)?;
        if let Some(snapshot) = &inspection.snapshot {
            check_outside_owned_directory(&destination, snapshot.path())?;
        }
        stage = Some(PrivateDirectory::new(
            parent(&destination),
            ".mio-ext-import-",
        )?);
        let staged_path = stage.as_ref().unwrap().path().join("project.mio");
        let mut store = ProjectStore::create(Some(&staged_path))?;
        store.cancel_queries_with(cancel.clone());
        let default_name = destination
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
            .map(str::to_owned);
        let source_id = Uuid::new_v4();
        let report = store.import_ext_file_streaming(
            source_id,
            &inspection.source_path,
            default_name,
            cancel,
            progress,
        )?;
        inspection.check_source()?;
        check_cancel(cancel)?;
        progress(ExtImportPhase::Publish, 0, Some(1));
        store.checkpoint()?;
        store.close()?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&staged_path)?;
        file.sync_all()?;
        let total_bytes = file.metadata()?.len();
        drop(file);
        inspection.check_source()?;
        check_cancel(cancel)?;
        check_new_project_path(&destination)?;
        if let Some(snapshot) = &inspection.snapshot {
            check_outside_owned_directory(&destination, snapshot.path())?;
        }
        fs::hard_link(&staged_path, &destination)
            .context("不能原子发布新项目；已有文件不会被覆盖")?;
        published = Some(destination.clone());
        #[cfg(unix)]
        File::open(parent(&destination))?
            .sync_all()
            .context("完整项目已经发布，但目录同步失败，尚不能确认掉电持久性")?;
        // A cancellation arriving after publication cannot undo a successfully created project.
        progress(ExtImportPhase::Publish, 1, Some(1));
        Ok((report, total_bytes))
    })();
    let stage_cleanup = stage.map(PrivateDirectory::close).transpose();
    let source_cleanup = inspection.release();
    let mut cleanup = CleanupFailure::default();
    for error in [stage_cleanup.as_ref().err(), source_cleanup.as_ref().err()]
        .into_iter()
        .flatten()
    {
        if let Some(pending) = error.downcast_ref::<CleanupFailure>() {
            cleanup.directories.extend(pending.directories.clone());
        }
    }
    let result = result.map_err(|error| {
        if cancel.is_cancelled()
            && matches!(error.downcast_ref::<rusqlite::Error>(), Some(rusqlite::Error::SqliteFailure(code, _)) if code.code == rusqlite::ErrorCode::OperationInterrupted)
        {
            Cancelled.into()
        } else { error }
    });
    let result = match (result, stage_cleanup, source_cleanup) {
        (result, Ok(_), Ok(())) => result,
        (result, stage, source) => Err(anyhow::anyhow!(
            "转换结果：{}；临时输出清理：{}；源快照清理：{}",
            result
                .as_ref()
                .err()
                .map_or("已完成".into(), |error| format!("{error:#}")),
            stage.err().map_or("完成".into(), |error| error.to_string()),
            source
                .err()
                .map_or("完成".into(), |error| format!("{error:#}")),
        )),
    };
    let outcome = match result {
        Ok((report, total_bytes)) => ExtJobOutcome::Imported {
            path: destination,
            report,
            total_bytes,
        },
        Err(error) => failed(error, published),
    };
    (outcome, (!cleanup.is_empty()).then_some(cleanup))
}
