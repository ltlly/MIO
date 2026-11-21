use super::*;

pub(crate) async fn serve_once(
    listener: TcpListener,
    response: Vec<u8>,
    gate: Option<ResponseGate>,
    requests: &AtomicUsize,
) -> Result<TcpListener> {
    let (mut stream, peer) = listener.accept().await?;
    ensure!(peer.ip().is_loopback(), "fixture peer is not loopback");
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        ensure!(head.len() < 4096, "fixture request head exceeded its bound");
        head.push(stream.read_u8().await?);
    }
    ensure!(
        head.starts_with(b"POST /repeater-compression HTTP/1.1\r\n"),
        "origin received the wrong request line"
    );
    let length = std::str::from_utf8(&head)?
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .context("origin request lacks Content-Length")?
        .1
        .trim()
        .parse::<usize>()?;
    ensure!(
        length == REQUEST_BODY.len(),
        "origin request length changed"
    );
    let mut body = [0; REQUEST_BODY.len()];
    stream.read_exact(&mut body).await?;
    ensure!(
        body.as_slice() == REQUEST_BODY,
        "origin request bytes changed"
    );
    requests.fetch_add(1, Ordering::SeqCst);
    if let Some((received, released)) = gate {
        received
            .send(())
            .map_err(|_| anyhow::anyhow!("request observer closed"))?;
        released.await.context("response gate closed")?;
    }
    stream.write_all(&response).await?;
    ensure!(
        stream.read(&mut [0]).await? == 0,
        "origin received another request on the same connection"
    );
    Ok(listener)
}

pub(crate) fn start_origin(
    listener: TcpListener,
    responses: Vec<Vec<u8>>,
    mut gate: Option<ResponseGate>,
    deadline: Instant,
) -> (JoinHandle<Result<TcpListener>>, Arc<AtomicUsize>) {
    let requests = Arc::new(AtomicUsize::new(0));
    let received = requests.clone();
    let origin = tokio::spawn(async move {
        timeout_at(deadline, async move {
            let mut listener = listener;
            for response in responses {
                listener = serve_once(listener, response, gate.take(), &received).await?;
            }
            Ok(listener)
        })
        .await
        .context("origin deadline")?
    });
    (origin, requests)
}

pub(crate) async fn reap(
    runtime: Runtime,
    client: Client,
    origin: JoinHandle<Result<TcpListener>>,
    requests: &AtomicUsize,
    expected_requests: usize,
    deadline: Instant,
) -> Result<()> {
    client.shutdown.cancel();
    drop(client);
    let mut shutdown = tokio::task::spawn_blocking(move || runtime.shutdown());
    let origin_result = origin.await;
    let (shutdown_result, within_deadline) = match timeout_at(deadline, &mut shutdown).await {
        Ok(result) => (result, Instant::now() <= deadline),
        // A deadline failure must not detach the Runtime's worker.
        Err(_) => (shutdown.await, false),
    };
    shutdown_result.context("join Runtime shutdown")??;
    let listener = origin_result.context("join origin")??.into_std()?;
    ensure!(
        within_deadline,
        "fixture and cleanup exceeded eight seconds"
    );
    ensure!(
        requests.load(Ordering::SeqCst) == expected_requests,
        "origin request count differs from explicit Send count"
    );
    ensure!(
        matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock),
        "no retry may remain queued after saving, reopening and Runtime shutdown"
    );
    Ok(())
}

pub(crate) async fn save_workspace(
    client: &mut Client,
    project: ProjectKey,
    workspace: &mut Workspace,
) -> Result<()> {
    workspace.revision += 1;
    client
        .commands
        .send(Command::SaveWorkspace {
            project,
            workspace: workspace.clone(),
        })
        .await?;
    let Event::WorkspaceChanged { workspace: saved, .. } = event(client, |event| {
        matches!(event, Event::WorkspaceChanged { project: owner, .. } if *owner == project)
    })
    .await?
    else {
        unreachable!()
    };
    ensure!(
        saved == *workspace,
        "SaveWorkspace ACK differs from the draft/settings"
    );
    Ok(())
}

pub(crate) async fn prepare(
    client: &mut Client,
    address: SocketAddr,
) -> Result<(ProjectKey, Workspace)> {
    let Event::ProjectOpened {
        project,
        mut workspace,
        ..
    } = event(client, |event| matches!(event, Event::ProjectOpened { .. })).await?
    else {
        unreachable!()
    };
    ensure!(
        workspace.repeater_settings.unpack_compressed_responses,
        "Repeater must unpack compressed responses by default"
    );
    client
        .commands
        .send(Command::NewRepeater { project })
        .await?;
    let Event::RepeaterOpened { mut tab, .. } = event(
        client,
        |event| matches!(event, Event::RepeaterOpened { project: owner, .. } if *owner == project),
    )
    .await?
    else {
        unreachable!()
    };
    tab.draft.target = format!("http://{address}");
    tab.draft.version = HttpVersion::Http11;
    tab.draft.raw = format!(
        "POST /repeater-compression HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\n\r\n",
        REQUEST_BODY.len()
    )
    .into_bytes();
    tab.draft.raw.extend_from_slice(REQUEST_BODY);
    tab.draft.revision += 1;
    workspace.active = Some(tab.id);
    workspace.tabs.push(tab);
    save_workspace(client, project, &mut workspace).await?;
    Ok((project, workspace))
}

pub(crate) async fn send(
    client: &mut Client,
    project: ProjectKey,
    tab: &WorkTab,
    settings: &RepeaterSettings,
) -> Result<()> {
    client
        .commands
        .send(Command::Send {
            run_id: mio_core::Uuid::new_v4(),
            cancel: tokio_util::sync::CancellationToken::new(),
            project,
            tab_id: tab.id,
            draft: tab.draft.clone(),
            settings: settings.clone(),
            group_send: None,
        })
        .await?;
    let Event::SendStarted { draft_revision, .. } = event(client, |event| {
        matches!(event, Event::SendStarted { project: owner, tab_id, .. }
            if *owner == project && *tab_id == tab.id)
    })
    .await?
    else {
        unreachable!()
    };
    ensure!(
        draft_revision == tab.draft.revision,
        "wrong started draft revision"
    );
    Ok(())
}

pub(crate) async fn read_flow(
    client: &mut Client,
    project: ProjectKey,
    sent: &FlowRecord,
) -> Result<OriginalMessages> {
    client
        .commands
        .send(Command::ReadFlow {
            project,
            selection_revision: 7,
            flow_id: sent.summary.id,
        })
        .await?;
    let Event::FlowDetail {
        record: stored,
        original,
        ..
    } = event(client, |event| {
        matches!(event, Event::FlowDetail { project: owner, selection_revision: 7, .. }
            if *owner == project)
    })
    .await?
    else {
        unreachable!()
    };
    ensure!(*stored == *sent, "ReadFlow must match SendFinished exactly");
    Ok(*original)
}

pub(crate) async fn finished(
    client: &mut Client,
    project: ProjectKey,
    tab: &WorkTab,
) -> Result<(FlowRecord, OriginalMessages)> {
    let Event::SendFinished {
        draft_revision,
        record: sent,
        persisted,
        ..
    } = event(client, |event| {
        matches!(event, Event::SendFinished { project: owner, tab_id, .. }
            if *owner == project && *tab_id == tab.id)
    })
    .await?
    else {
        unreachable!()
    };
    ensure!(
        draft_revision == tab.draft.revision,
        "wrong sent draft revision"
    );
    ensure!(persisted, "SendFinished was not persisted");
    ensure!(sent.summary.source == TrafficSource::Repeater, "wrong tool");
    ensure!(
        sent.request.method == "POST",
        "stored request method changed"
    );
    ensure!(
        sent.request.url == format!("{}/repeater-compression", tab.draft.target),
        "stored request URL changed"
    );
    ensure!(
        sent.request.version == HttpVersion::Http11,
        "stored request version changed"
    );
    ensure!(
        sent.request.body == REQUEST_BODY,
        "stored request body changed"
    );
    ensure!(
        sent.summary.request_bytes == REQUEST_BODY.len() as u64,
        "wrong request byte count"
    );
    let response = sent.response.as_ref().context("SendFinished response")?;
    ensure!(
        sent.summary.status == Some(response.status),
        "summary status differs"
    );
    ensure!(
        sent.summary.response_bytes == response.body.len() as u64,
        "wrong response byte count"
    );
    let original = read_flow(client, project, &sent).await?;
    Ok((*sent, original))
}
