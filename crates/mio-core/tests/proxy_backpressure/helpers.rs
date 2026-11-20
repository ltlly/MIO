pub(crate) use std::{io::ErrorKind, net::SocketAddr, path::PathBuf, time::Duration};

pub(crate) use mio_core::{
    Client, Command, Decision, Event, FlowRecord, FlowState, FlowSummary, HttpResponse,
    HttpVersion, InterceptStage, OriginalMessages, ProjectKey, ProxyInterceptOperator,
    ProxyInterceptRelation, ProxyInterceptRule, ProxyInterceptType, Runtime, ScopeRule, Uuid,
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
    client
        .events
        .recv()
        .await
        .expect("the service stays connected")
}

pub(crate) async fn event(client: &mut Client, predicate: impl Fn(&Event) -> bool) -> Event {
    loop {
        let event = next(client).await;
        assert!(!matches!(event, Event::Error { .. }), "{event:?}");
        if predicate(&event) {
            return event;
        }
    }
}

pub(crate) async fn opened(client: &mut Client) -> ProjectKey {
    let Event::ProjectOpened { project, .. } =
        event(client, |event| matches!(event, Event::ProjectOpened { .. })).await
    else {
        unreachable!()
    };
    assert!(matches!(next(client).await, Event::ProxyState {
        project: owner, address: None, cancel: None, ..
    } if owner == project));
    project
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

pub(crate) async fn page(
    client: &mut Client,
    project: ProjectKey,
    query_revision: u64,
) -> Vec<FlowSummary> {
    client
        .commands
        .send(Command::QueryHistory {
            project,
            query_revision,
            offset: 0,
            limit: 2,
            filter: String::new(),
        })
        .await
        .unwrap();
    let Event::HistoryPage { total, rows, .. } = event(client, |event| {
        matches!(event,
            Event::HistoryPage { project: owner, query_revision: revision, .. }
                if *owner == project && *revision == query_revision
        )
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
                panic!("loopback capture fixture must contain only live history")
            }
        })
        .collect()
}

pub(crate) async fn detail(
    client: &mut Client,
    project: ProjectKey,
    summary: &FlowSummary,
) -> FlowRecord {
    let (record, original) = detail_with_original(client, project, summary).await;
    assert_eq!(original, Default::default());
    record
}

pub(crate) async fn detail_with_original(
    client: &mut Client,
    project: ProjectKey,
    summary: &FlowSummary,
) -> (FlowRecord, OriginalMessages) {
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
        Event::FlowDetail { project: owner, selection_revision, record, .. }
            if *owner == project && *selection_revision == summary.revision && record.summary.id == summary.id
    )).await else { unreachable!() };
    (*record, *original)
}

pub(crate) struct Fixture {
    pub(crate) runtime: Runtime,
    pub(crate) client: Client,
    pub(crate) directory: tempfile::TempDir,
    pub(crate) path: PathBuf,
    pub(crate) project: ProjectKey,
    pub(crate) listener: TcpListener,
    pub(crate) origin: TcpStream,
    pub(crate) downstream: TcpStream,
    pub(crate) address: SocketAddr,
    pub(crate) cancel: CancellationToken,
    pub(crate) initial: FlowSummary,
}

impl Fixture {
    pub(crate) async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("proxy-backpressure.mio");
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = opened(&mut client).await;
        save(&mut client, project, Some(path.clone())).await;
        client
            .commands
            .send(Command::StartProxy {
                project,
                listen: "127.0.0.1:0".parse().unwrap(),
            })
            .await
            .unwrap();
        let Event::ProxyState {
            address: Some(address),
            cancel: Some(cancel),
            ..
        } = event(&mut client, |event| {
            matches!(
                event,
                Event::ProxyState {
                    address: Some(_),
                    ..
                }
            )
        })
        .await
        else {
            panic!("the listening state must carry the active Proxy cancellation token")
        };
        assert!(!cancel.is_cancelled());
        assert!(page(&mut client, project, 1).await.is_empty());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap();
        let mut downstream = TcpStream::connect(address).await.unwrap();
        downstream.write_all(format!(
            "POST http://{target}/once HTTP/1.1\r\nHost: {target}\r\nContent-Length: {}\r\n\r\n",
            BODY.len(),
        ).as_bytes()).await.unwrap();
        downstream.write_all(BODY).await.unwrap();
        let (mut origin, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            assert!(head.len() < 8192);
            head.push(origin.read_u8().await.unwrap());
        }
        assert!(head.starts_with(b"POST /once HTTP/1.1\r\n"));
        let mut body = vec![0; BODY.len()];
        origin.read_exact(&mut body).await.unwrap();
        assert_eq!(body, BODY);
        event(&mut client, |event| {
            matches!(event, Event::HistoryChanged { .. })
        })
        .await;
        let initial = page(&mut client, project, 2).await.pop().unwrap();
        assert_eq!(initial.state, FlowState::Pending);
        assert_eq!(initial.request_bytes, BODY.len() as u64);
        assert!(client.events.try_recv().is_err());
        Self {
            runtime,
            client,
            directory,
            path,
            project,
            listener,
            origin,
            downstream,
            address,
            cancel,
            initial,
        }
    }

    pub(crate) async fn block_history_reply(&mut self) {
        assert_eq!(self.client.events.max_capacity(), 128);
        for query_revision in 0..128 {
            self.client
                .commands
                .send(Command::QueryHistory {
                    project: self.project,
                    query_revision,
                    offset: 0,
                    limit: 1,
                    filter: String::new(),
                })
                .await
                .unwrap();
        }
        while self.client.events.len() != 128 {
            tokio::task::yield_now().await;
        }
        self.client
            .commands
            .send(Command::QueryHistory {
                project: self.project,
                query_revision: WAITING_QUERY,
                offset: 0,
                limit: 1,
                filter: String::new(),
            })
            .await
            .unwrap();
        while self.client.commands.capacity() != self.client.commands.max_capacity() {
            tokio::task::yield_now().await;
        }
        assert_eq!(self.client.events.len(), 128);
    }

    pub(crate) fn stored(&self, connection: &Connection) -> FlowSummary {
        let json: Vec<u8> = connection
            .query_row(
                "SELECT summary FROM flows WHERE id=?1",
                [self.initial.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_slice(&json).unwrap()
    }
}

pub(crate) fn no_extra_connection(listener: TcpListener) {
    let listener = listener.into_std().unwrap();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        ErrorKind::WouldBlock,
        "the POST must never be retried"
    );
}
