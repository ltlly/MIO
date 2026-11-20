pub(crate) use std::{io::ErrorKind, path::PathBuf, time::Duration};

pub(crate) use mio_core::{
    Client, Command, Draft, Event, FlowState, HttpVersion, IntruderDiscardReceipt,
    IntruderResultOutcome, IntruderResultPage, IntruderRunState, IntruderSpec, IntruderTab,
    ProjectKey, Runtime, Uuid, Workspace,
    intruder::{AttackType, Position},
};
pub(crate) use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
pub(crate) use tokio_util::sync::CancellationToken;

pub(crate) async fn event(client: &mut Client, matches: impl Fn(&Event) -> bool) -> Event {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = client
                .events
                .recv()
                .await
                .expect("service remains connected");
            if matches(&event) {
                return event;
            }
            assert!(
                !matches!(event, Event::Error { .. }),
                "unexpected error: {event:?}"
            );
        }
    })
    .await
    .expect("bounded discard workflow reply")
}

pub(crate) async fn opened(client: &mut Client) -> (ProjectKey, Workspace) {
    match event(client, |event| matches!(event, Event::ProjectOpened { .. })).await {
        Event::ProjectOpened {
            project, workspace, ..
        } => (project, workspace),
        _ => unreachable!(),
    }
}

pub(crate) async fn page(
    client: &mut Client,
    project: ProjectKey,
    run_id: Uuid,
    revision: u64,
) -> Result<IntruderResultPage, String> {
    client
        .commands
        .send(Command::QueryIntruderResults {
            project,
            run_id,
            query_revision: revision,
            after: None,
            limit: 2,
        })
        .await
        .unwrap();
    match event(client, |event| {
        matches!(event, Event::IntruderResults {
        project: owner, run_id: id, query_revision, ..
    } if *owner == project && *id == run_id && *query_revision == revision)
    })
    .await
    {
        Event::IntruderResults { result, .. } => result,
        _ => unreachable!(),
    }
}

pub(crate) async fn terminal(
    client: &mut Client,
    project: ProjectKey,
    run_id: Uuid,
) -> IntruderResultPage {
    let mut revision = 1;
    loop {
        let page = page(client, project, run_id, revision).await.unwrap();
        assert!(page.persisted);
        if page.run.state.is_terminal() {
            return page;
        }
        event(
            client,
            |event| matches!(event, Event::IntruderChanged { project: owner } if *owner == project),
        )
        .await;
        revision += 1;
    }
}

pub(crate) async fn discard(
    client: &mut Client,
    project: ProjectKey,
    run_id: Uuid,
    operation_id: Uuid,
    revision: u64,
    workspace: Workspace,
) -> Result<IntruderDiscardReceipt, String> {
    client
        .commands
        .send(Command::DiscardIntruder {
            project,
            run_id,
            operation_id,
            expected_revision: revision,
            workspace: Box::new(workspace),
        })
        .await
        .unwrap();
    match event(client, |event| {
        matches!(event, Event::IntruderDiscarded {
        project: owner, run_id: id, operation_id: operation, ..
    } if *owner == project && *id == run_id && *operation == operation_id)
    })
    .await
    {
        Event::IntruderDiscarded { result, .. } => result,
        _ => unreachable!(),
    }
}

pub(crate) async fn save(
    client: &mut Client,
    project: ProjectKey,
    path: Option<PathBuf>,
    workspace: Option<Workspace>,
) {
    client
        .commands
        .send(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project,
            path: path.clone(),
            workspace,
        })
        .await
        .unwrap();
    match event(client, |event| matches!(event, Event::ProjectSaved { .. })).await {
        Event::ProjectSaved {
            project: owner,
            path: saved,
            ..
        } => {
            assert_eq!(owner, project);
            if let Some(path) = path {
                assert_eq!(saved, path);
            }
        }
        _ => unreachable!(),
    }
}

pub(crate) async fn request(stream: &mut TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let count = stream.read_buf(&mut bytes).await.unwrap();
        assert!(count > 0 && bytes.len() < 8192);
        if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            let head = std::str::from_utf8(&bytes[..end]).unwrap();
            let length = head
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .unwrap()
                .1
                .trim()
                .parse::<usize>()
                .unwrap();
            assert!(bytes.len() <= end + 4 + length);
            if bytes.len() == end + 4 + length {
                assert_eq!(
                    head.lines()
                        .filter_map(|line| line.split_once(':'))
                        .filter(|(name, _)| name.eq_ignore_ascii_case("x-repeat"))
                        .map(|(_, value)| value.trim())
                        .collect::<Vec<_>>(),
                    ["one", "two"]
                );
                return bytes[end + 4..].to_vec();
            }
        }
    }
}
