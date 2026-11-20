use super::*;

pub(crate) async fn event(client: &mut Client, accept: impl Fn(&Event) -> bool) -> Result<Event> {
    loop {
        let event = client
            .events
            .recv()
            .await
            .context("backend event channel closed")?;
        if let Event::Error { operation, .. } = &event {
            bail!(
                "backend operation {operation} failed; details suppressed to avoid traffic disclosure"
            );
        }
        ensure!(
            !matches!(event, Event::Stopped),
            "backend stopped before the requested acknowledgement"
        );
        if accept(&event) {
            return Ok(event);
        }
    }
}

pub(crate) async fn send(client: &Client, command: Command) -> Result<()> {
    client
        .commands
        .send(command)
        .await
        .map_err(|_| anyhow::anyhow!("backend command channel closed"))
}

pub(crate) async fn opened(
    client: &mut Client,
    expected: Option<&Path>,
) -> Result<(ProjectKey, Workspace)> {
    let Event::ProjectOpened {
        project,
        path,
        workspace,
    } =
        event(client, |event| matches!(event, Event::ProjectOpened { .. })).await?
    else {
        unreachable!("event predicate")
    };
    ensure!(
        path.as_deref() == expected,
        "opened project path differs from requested path"
    );
    Ok((project, workspace))
}

pub(crate) async fn job(
    client: &mut Client,
    project: ProjectKey,
    job_id: Uuid,
    output: &mut impl Write,
) -> Result<ExtJobOutcome> {
    let Event::ExtJob { outcome, .. } = event(client, |event| matches!(event, Event::ExtJob { project: owner, job_id: id, .. } if *owner == project && *id == job_id)).await? else {
        unreachable!("event predicate")
    };
    match &outcome {
        ExtJobOutcome::Failed { created_path, .. } => {
            if let Some(path) = created_path {
                writeln!(
                    output,
                    "publication=created_but_failed created_path={}",
                    path.display()
                )?;
            }
            bail!("External job failed; backend message suppressed to avoid traffic disclosure");
        }
        ExtJobOutcome::Cancelled => bail!("External job acknowledged cancellation"),
        _ => Ok(outcome),
    }
}

pub(crate) async fn snapshot(
    client: &mut Client,
    project: ProjectKey,
    workspace: &Workspace,
    revision: &mut u64,
) -> Result<Snapshot> {
    *revision += 1;
    send(
        client,
        Command::QueryExtReport {
            project,
            query_revision: *revision,
        },
    )
    .await?;
    let Event::ExtReport { result, .. } = event(client, |event| matches!(event, Event::ExtReport { project: owner, query_revision, .. } if *owner == project && *query_revision == *revision)).await? else {
        unreachable!("event predicate")
    };
    let source = result
        .map_err(|_| anyhow::anyhow!("cannot read persisted import report"))?
        .context("project has no persisted External source")?;
    ensure!(
        source.report.complete_archive,
        "archive is not marked complete"
    );
    *revision += 1;
    let query = HttpHistoryQuery {
        project,
        query_revision: *revision,
        offset: 0,
        limit: PAGE.min(500),
        url_filter: String::new(),
        filter: HttpHistoryFilter::default(),
        sort: HttpHistorySort::default(),
        selected_ids: Vec::new(),
    };
    let expected_rev = *revision;
    send(client, Command::QueryHttpHistory { query }).await?;
    let Event::HttpHistoryPage { result, .. } = event(client, |event| {
        matches!(
            event,
            Event::HttpHistoryPage { query, .. }
                if query.project == project && query.query_revision == expected_rev
        )
    })
    .await?
    else {
        unreachable!("event predicate")
    };
    let hist = result.map_err(|_| anyhow::anyhow!("cannot read imported HTTP history"))?;
    Ok(Snapshot {
        source,
        proxy_flows: hist.total,
        repeater_tabs: workspace.tabs.len(),
        repeater_groups: workspace.groups.len(),
    })
}
