pub(crate) use std::{io::ErrorKind, path::PathBuf, time::Duration};

pub(crate) use mio_core::{
    Client, Command, Event, FlowId, FlowRecord, FlowState, FlowSummary, ProjectKey, Runtime, Uuid,
    Workspace,
};
pub(crate) use rusqlite::{Connection, OpenFlags};
pub(crate) use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
pub(crate) use tokio_util::sync::CancellationToken;

pub(crate) const FIXTURE_BUDGET: Duration = Duration::from_secs(8);
pub(crate) const OBSERVATION_BUDGET: Duration = Duration::from_secs(3);
pub(crate) const BODY: &[u8] = &[0, 0xff, b'P', b'O', b'S', b'T'];
pub(crate) const WAITING_QUERY: u64 = u64::MAX;

pub(crate) async fn next(client: &mut Client) -> Event {
    loop {
        let event = client
            .events
            .recv()
            .await
            .expect("the service stays connected");
        assert!(
            !matches!(
                event,
                Event::Error { .. } | Event::SendRejected { .. } | Event::WorkspaceRejected { .. }
            ),
            "{event:?}"
        );
        if matches!(event, Event::ProjectDiskUsage { .. }) {
            continue;
        }
        return event;
    }
}

pub(crate) async fn event(client: &mut Client, predicate: impl Fn(&Event) -> bool) -> Event {
    loop {
        let event = next(client).await;
        if predicate(&event) {
            return event;
        }
    }
}

pub(crate) async fn opened(client: &mut Client) -> (ProjectKey, Workspace) {
    let Event::ProjectOpened {
        project, workspace, ..
    } = event(client, |event| matches!(event, Event::ProjectOpened { .. })).await
    else {
        unreachable!();
    };
    assert!(matches!(next(client).await, Event::ProxyState {
        project: owner, address: None, cancel: None, ..
    } if owner == project));
    (project, workspace)
}

pub(crate) async fn save(client: &mut Client, project: ProjectKey, path: Option<PathBuf>) {
    client
        .commands
        .send(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project,
            path,
            workspace: None,
        })
        .await
        .unwrap();
    event(
        client,
        |event| matches!(event, Event::ProjectSaved { project: owner, .. } if *owner == project),
    )
    .await;
}

pub(crate) async fn save_workspace(
    client: &mut Client,
    project: ProjectKey,
    workspace: &Workspace,
) {
    client
        .commands
        .send(Command::SaveWorkspace {
            project,
            workspace: workspace.clone(),
        })
        .await
        .unwrap();
    assert!(
        matches!(event(client, |event| matches!(event, Event::WorkspaceChanged { .. })).await,
        Event::WorkspaceChanged { project: owner, workspace: saved } if owner == project && saved == *workspace)
    );
}

pub(crate) async fn page(
    client: &mut Client,
    project: ProjectKey,
    revision: u64,
) -> Vec<FlowSummary> {
    client
        .commands
        .send(Command::QueryHistory {
            project,
            query_revision: revision,
            offset: 0,
            limit: 16,
            filter: String::new(),
        })
        .await
        .unwrap();
    let Event::HistoryPage { total, rows, .. } = event(client, |event| {
        matches!(event,
        Event::HistoryPage { project: owner, query_revision, .. }
            if *owner == project && *query_revision == revision)
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(total, rows.len());
    rows.into_iter()
        .map(|row| match row {
            mio_core::HistoryRow::Live(summary) => summary,
            mio_core::HistoryRow::ExtProxy(_) => {
                panic!("Repeater send fixture must contain only live history")
            }
        })
        .collect()
}

pub(crate) async fn detail(
    client: &mut Client,
    project: ProjectKey,
    summary: &FlowSummary,
) -> FlowRecord {
    client
        .commands
        .send(Command::ReadFlow {
            project,
            selection_revision: summary.revision,
            flow_id: summary.id,
        })
        .await
        .unwrap();
    let Event::FlowDetail { record, original, .. } = event(client, |event| matches!(event,
        Event::FlowDetail { project: owner, record, selection_revision, .. }
            if *owner == project && record.summary.id == summary.id && *selection_revision == summary.revision)).await else { unreachable!() };
    assert_eq!(*original, Default::default());
    assert_eq!(&record.summary, summary);
    *record
}

pub(crate) struct Fixture {
    pub(crate) runtime: Runtime,
    pub(crate) client: Client,
    pub(crate) directory: tempfile::TempDir,
    pub(crate) path: PathBuf,
    pub(crate) project: ProjectKey,
    pub(crate) workspace: Workspace,
    pub(crate) listener: TcpListener,
    pub(crate) run_id: FlowId,
    pub(crate) cancel: CancellationToken,
}

impl Fixture {
    pub(crate) async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("repeater-backpressure.mio");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (runtime, mut client) = Runtime::start().unwrap();
        let (project, mut workspace) = opened(&mut client).await;
        client
            .commands
            .send(Command::NewRepeater { project })
            .await
            .unwrap();
        let Event::RepeaterOpened { mut tab, .. } = next(&mut client).await else {
            panic!("NewRepeater returns an unsent tab");
        };
        tab.draft.target = format!("http://{address}");
        tab.draft.raw = format!(
            "POST /once HTTP/1.1\r\nHost: {address}\r\nX-Duplicate: one\r\nX-Duplicate: two\r\n\r\n"
        )
        .into_bytes();
        tab.draft.raw.extend_from_slice(BODY);
        tab.draft.revision += 1;
        workspace.active = Some(tab.id);
        workspace.tabs.push(tab);
        workspace.revision += 1;
        save_workspace(&mut client, project, &workspace).await;
        save(&mut client, project, Some(path.clone())).await;
        assert!(page(&mut client, project, 1000).await.is_empty());
        assert!(client.events.try_recv().is_err());
        Self {
            runtime,
            client,
            directory,
            path,
            project,
            workspace,
            listener,
            run_id: Uuid::new_v4(),
            cancel: CancellationToken::new(),
        }
    }

    pub(crate) async fn send(&mut self) {
        let tab = &self.workspace.tabs[0];
        self.client
            .commands
            .send(Command::Send {
                project: self.project,
                tab_id: tab.id,
                run_id: self.run_id,
                draft: tab.draft.clone(),
                settings: self.workspace.repeater_settings.clone(),
                group_send: None,
                cancel: self.cancel.clone(),
            })
            .await
            .unwrap();
    }

    pub(crate) async fn receive_request(&self) -> TcpStream {
        let (mut stream, _) = self.listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            assert!(head.len() < 8192);
            head.push(stream.read_u8().await.unwrap());
        }
        assert!(head.starts_with(b"POST /once HTTP/1.1\r\n"));
        assert_eq!(
            String::from_utf8(head)
                .unwrap()
                .to_ascii_lowercase()
                .matches("\r\nx-duplicate:")
                .count(),
            2
        );
        let mut body = vec![0; BODY.len()];
        stream.read_exact(&mut body).await.unwrap();
        assert_eq!(body, BODY);
        stream
    }

    pub(crate) async fn fill_events(&mut self) {
        assert_eq!(self.client.events.max_capacity(), 128);
        for query_revision in 0..128 {
            self.client
                .commands
                .send(Command::QueryHistory {
                    project: self.project,
                    query_revision,
                    offset: 0,
                    limit: 16,
                    filter: String::new(),
                })
                .await
                .unwrap();
        }
        while self.client.events.len() != 128 {
            tokio::task::yield_now().await;
        }
    }

    pub(crate) fn stored(connection: &Connection) -> FlowSummary {
        let json: Vec<u8> = connection
            .query_row("SELECT summary FROM flows", [], |row| row.get(0))
            .unwrap();
        serde_json::from_slice(&json).unwrap()
    }

    pub(crate) fn stored_workspace(connection: &Connection) -> Workspace {
        let json: Vec<u8> = connection
            .query_row(
                "SELECT value FROM metadata WHERE key='workspace'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_slice(&json).unwrap()
    }

    pub(crate) async fn save_and_reopen(&mut self, record: &FlowRecord) {
        self.workspace.tabs[0].last_flow = Some(record.summary.id);
        self.workspace.revision += 1;
        save_workspace(&mut self.client, self.project, &self.workspace).await;
        save(&mut self.client, self.project, None).await;
        let copy = self.directory.path().join("repeater-copy.mio");
        save(&mut self.client, self.project, Some(copy.clone())).await;
        for path in [self.path.clone(), copy] {
            self.client
                .commands
                .send(Command::NewProject { path: None })
                .await
                .unwrap();
            opened(&mut self.client).await;
            self.client
                .commands
                .send(Command::OpenProject { path })
                .await
                .unwrap();
            let (reopened, workspace) = opened(&mut self.client).await;
            assert_eq!(reopened.id, self.project.id);
            assert!(reopened.generation > self.project.generation);
            assert_eq!(workspace, self.workspace);
            assert_eq!(
                page(&mut self.client, reopened, 500).await.as_slice(),
                std::slice::from_ref(&record.summary)
            );
            assert_eq!(
                detail(&mut self.client, reopened, &record.summary).await,
                *record
            );
        }
    }

    pub(crate) async fn shutdown(self) {
        tokio::task::spawn_blocking(move || self.runtime.shutdown())
            .await
            .unwrap()
            .unwrap();
        let listener = self.listener.into_std().unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            ErrorKind::WouldBlock,
            "cancel/save/reopen must not resend POST"
        );
    }
}
