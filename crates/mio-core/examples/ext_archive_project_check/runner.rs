use super::*;

pub(crate) async fn workflow(
    client: &mut Client,
    paths: &Paths,
    cancel: &CancellationToken,
    output: &mut impl Write,
) -> Result<()> {
    let mut started = Instant::now();
    let (project, _) = opened(client, None).await?;
    let inspection_job = Uuid::new_v4();
    send(
        client,
        Command::InspectExternal {
            project,
            job_id: inspection_job,
            path: paths.source.clone(),
            cancel: cancel.clone(),
        },
    )
    .await?;
    let ExtJobOutcome::Inspected(inspection) =
        job(client, project, inspection_job, output).await?
    else {
        bail!("inspection returned an unexpected terminal kind");
    };
    phase(output, "inspect", &mut started)?;
    new_path(&paths.converted)?;
    let conversion_job = Uuid::new_v4();
    send(
        client,
        Command::ConvertExternal {
            project,
            job_id: conversion_job,
            inspection_id: inspection.id,
            destination: paths.converted.clone(),
            cancel: cancel.clone(),
        },
    )
    .await?;
    let ExtJobOutcome::Imported { path, report, .. } =
        job(client, project, conversion_job, output).await?
    else {
        bail!("conversion returned an unexpected terminal kind");
    };
    ensure!(
        path == paths.converted,
        "published path differs from the requested new target"
    );
    writeln!(
        output,
        "publication=created created_path={}",
        path.display()
    )?;
    phase(output, "convert", &mut started)?;
    send(client, Command::OpenProject { path }).await?;
    let (project, workspace) = opened(client, Some(&paths.converted)).await?;
    phase(output, "open", &mut started)?;
    let mut revision = 0;
    let before = snapshot(client, project, &workspace, &mut revision).await?;
    ensure!(
        before.source.report == report,
        "persisted report differs from conversion result"
    );
    let (coverage, _) = mio_core::inspect_mio_ext_archive(&paths.converted, 0, 50)?
        .context("converted project is missing its External archive")?;
    ensure!(
        coverage.categories.len() == 6,
        "expected 6 External coverage categories"
    );
    phase(output, "report_pages_samples", &mut started)?;
    writeln!(
        output,
        "schema={}/{} indexed_items={} messages={} proxy_flows={} repeater_tabs={} repeater_groups={} collections={} full_project_compatibility=unverified",
        report.schema_floor,
        report.schema_revision,
        report.indexed_items,
        report.indexed_messages,
        before.proxy_flows,
        before.repeater_tabs,
        before.repeater_groups,
        report.collections.len(),
    )?;
    for (stage, target) in [("save", None), ("save_as", Some(&paths.saved))] {
        if let Some(target) = target {
            new_path(target)?;
        }
        send(
            client,
            Command::SaveProject {
                websocket_notes: Vec::new(),
                ext_http_notes: Vec::new(),
                ext_http_highlights: Vec::new(),
                project,
                path: target.cloned(),
                workspace: None,
            },
        )
        .await?;
        let Event::ProjectSaved { path, .. } = event(client, |event| matches!(event, Event::ProjectSaved { project: owner, .. } if *owner == project)).await? else {
            unreachable!("event predicate")
        };
        ensure!(
            path == *target.unwrap_or(&paths.converted),
            "save acknowledged another path"
        );
        phase(output, stage, &mut started)?;
    }
    send(client, Command::NewProject { path: None }).await?;
    opened(client, None).await?;
    phase(output, "new_project", &mut started)?;
    send(
        client,
        Command::OpenProject {
            path: paths.saved.clone(),
        },
    )
    .await?;
    let (reopened, reopened_ws) = opened(client, Some(&paths.saved)).await?;
    ensure!(
        reopened.id == project.id && reopened != project,
        "project identity/generation did not survive reopen correctly"
    );
    phase(output, "reopen", &mut started)?;
    let after = snapshot(client, reopened, &reopened_ws, &mut revision).await?;
    ensure!(
        before == after,
        "source identity, report, proxy flows or repeater workspace changed after save-as/reopen"
    );
    phase(output, "recheck", &mut started)?;
    Ok(())
}

pub(crate) fn disk_usage(output: &mut impl Write, label: &str, path: &Path) -> Result<bool> {
    let mut sizes = [0; 4];
    let mut present = false;
    for (index, suffix) in ["", "-wal", "-shm", "-journal"].iter().enumerate() {
        let mut candidate = path.as_os_str().to_owned();
        candidate.push(suffix);
        match fs::symlink_metadata(Path::new(&candidate)) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_file(),
                    "project output was replaced by a non-file"
                );
                sizes[index] = metadata.len();
                if index == 0 {
                    present = true;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("cannot measure project output"),
        }
    }
    // Presence alone never claims that publication completed or that this process owns the file.
    writeln!(
        output,
        "output={label} path={} file_present={present} file_bytes={} wal_bytes={} shm_bytes={} journal_bytes={} total_bytes={}",
        path.display(),
        sizes[0],
        sizes[1],
        sizes[2],
        sizes[3],
        sizes.iter().sum::<u64>()
    )?;
    Ok(present)
}

pub(crate) fn check(paths: &Paths, output: &mut impl Write, work_budget: Duration) -> Result<()> {
    let started = Instant::now();
    let source = Source::open(&paths.source)?;
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (runtime, mut client) = Runtime::start()
        .map_err(|_| anyhow::anyhow!("runtime startup failed; backend details suppressed"))?;
    let cancel = CancellationToken::new();
    let result = executor.block_on(async {
        tokio::time::timeout(
            work_budget,
            workflow(&mut client, paths, &cancel, output),
        )
        .await
        .context("work deadline exceeded; cancellation requested, cleanup still requires join")?
    });
    cancel.cancel();
    client.shutdown.cancel();
    drop(client);
    let mut shutdown_started = Instant::now();
    let shutdown = runtime.shutdown().map_err(|_| {
        anyhow::anyhow!(
            "runtime shutdown/join failed; cleanup is not confirmed; backend details suppressed"
        )
    });
    phase(output, "shutdown_join", &mut shutdown_started)?;
    writeln!(
        output,
        "runtime_join={}",
        if shutdown.is_ok() {
            "confirmed"
        } else {
            "unconfirmed"
        }
    )?;
    let source_state = source.unchanged(&paths.source);
    writeln!(
        output,
        "source_metadata={}",
        if source_state.is_ok() {
            "unchanged"
        } else {
            "changed_or_unconfirmed"
        }
    )?;
    let inventory = (|| {
        let converted = disk_usage(output, "converted", &paths.converted)?;
        let saved = disk_usage(output, "save_as", &paths.saved)?;
        Ok::<_, anyhow::Error>((converted, saved))
    })();
    let elapsed = started.elapsed();
    writeln!(
        output,
        "total_wall_ms={} budget_exceeded={}",
        elapsed.as_millis(),
        elapsed > TOTAL_BUDGET
    )?;
    shutdown?;
    result?;
    source_state?;
    let (converted, saved) = inventory?;
    ensure!(
        converted && saved,
        "an acknowledged project output is missing"
    );
    // Blocking filesystem calls are not hard-interruptible; never advertise a hard timeout.
    ensure!(
        elapsed <= TOTAL_BUDGET,
        "verification exceeded the ten-minute budget"
    );
    writeln!(
        output,
        "verification=passed load_measurement=warm_or_unspecified performance_parity=unverified"
    )?;
    Ok(())
}
