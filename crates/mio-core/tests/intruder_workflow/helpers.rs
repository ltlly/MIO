pub(crate) use std::{io::ErrorKind, time::Duration};

pub(crate) use mio_core::{
    Client, Command, Draft, Event, FlowState, HttpVersion, IntruderResultOutcome,
    IntruderResultPage, IntruderRunState, IntruderRunSummary, IntruderSpec, IntruderTab,
    ProjectKey, Runtime, Uuid, Workspace,
    intruder::{AttackType, Position},
};
pub(crate) use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};
pub(crate) use tokio_util::sync::CancellationToken;

pub(crate) const DEADLINE: Duration = Duration::from_secs(8);

pub(crate) async fn event(client: &mut Client, predicate: impl Fn(&Event) -> bool) -> Event {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let event = client
                .events
                .recv()
                .await
                .expect("service should remain connected");
            if predicate(&event) {
                return event;
            }
            assert!(
                !matches!(event, Event::Error { .. }),
                "unexpected service error: {event:?}"
            );
        }
    })
    .await
    .expect("bounded causal Intruder event")
}

pub(crate) async fn opened(client: &mut Client) -> ProjectKey {
    opened_workspace(client).await.0
}

pub(crate) async fn opened_workspace(client: &mut Client) -> (ProjectKey, Workspace) {
    match event(client, |e| matches!(e, Event::ProjectOpened { .. })).await {
        Event::ProjectOpened {
            project, workspace, ..
        } => (project, workspace),
        _ => unreachable!(),
    }
}

pub(crate) fn spec(target: String, payloads: Vec<Vec<u8>>) -> IntruderSpec {
    let raw = b"POST /intruder HTTP/1.1\r\nHost: original.invalid\r\nX-Repeat: one\r\nX-Repeat: two\r\nContent-Length: 1\r\n\r\nX".to_vec();
    let position = Position {
        start: raw.len() - 1,
        end: raw.len(),
    };
    IntruderSpec {
        resource_pool: None,
        baseline_request: false,
        revision: 7,
        draft: Draft {
            revision: 3,
            target,
            raw,
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![position],
        payload_sets: vec![payloads],
        // Explicit fixture settings, not native External defaults.
        max_concurrent_requests: 1,
        request_timeout_ms: 5_000,
        update_host_header: true,
        save_to_project: true,
    }
}

pub(crate) async fn start(
    client: &mut Client,
    project: ProjectKey,
    run_id: Uuid,
    tab_id: Uuid,
    spec: &IntruderSpec,
    cancel: CancellationToken,
) -> Result<IntruderRunSummary, String> {
    client
        .commands
        .send(Command::StartIntruder {
            project,
            run_id,
            tab_id,
            spec: Box::new(spec.clone()),
            cancel,
        })
        .await
        .unwrap();
    match event(client, |e| {
        matches!(e, Event::IntruderStarted { project: key, run_id: id, .. }
            if *key == project && *id == run_id)
    })
    .await
    {
        Event::IntruderStarted {
            tab_id: actual,
            spec_revision,
            result,
            ..
        } => {
            assert_eq!(actual, tab_id);
            assert_eq!(spec_revision, spec.revision);
            if let Ok(run) = &result {
                assert_eq!((run.id, run.tab_id), (run_id, tab_id));
                assert_eq!(run.spec_revision, spec.revision);
            }
            result
        }
        _ => unreachable!(),
    }
}

pub(crate) async fn set_paused(
    client: &mut Client,
    project: ProjectKey,
    run_id: Uuid,
    paused: bool,
) -> Result<IntruderRunSummary, String> {
    let operation_id = Uuid::new_v4();
    client
        .commands
        .send(Command::SetIntruderPaused {
            project,
            run_id,
            operation_id,
            paused,
        })
        .await
        .unwrap();
    match event(client, |event| {
        matches!(event, Event::IntruderRunControl {
            project: key, run_id: id, operation_id: operation, ..
        } if *key == project && *id == run_id && *operation == operation_id)
    })
    .await
    {
        Event::IntruderRunControl { result, .. } => result,
        _ => unreachable!(),
    }
}

pub(crate) async fn bind(
    client: &mut Client,
    project: ProjectKey,
    workspace: &mut Workspace,
    spec: &IntruderSpec,
) -> Uuid {
    let id = Uuid::new_v4();
    workspace.intruder.tabs.push(IntruderTab {
        id,
        title: "fixture".into(),
        group: None,
        spec: spec.clone(),
        last_run: None,
    });
    workspace.intruder.active = Some(id);
    workspace.revision += 1;
    save_workspace(client, project, workspace).await;
    id
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
    match event(client, |event| {
        matches!(
            event,
            Event::WorkspaceChanged { .. } | Event::WorkspaceRejected { .. }
        )
    })
    .await
    {
        Event::WorkspaceChanged {
            project: key,
            workspace: saved,
        } => {
            assert_eq!(key, project);
            assert_eq!(saved, *workspace);
        }
        unexpected => panic!("fixture workspace must be acknowledged: {unexpected:?}"),
    }
}

pub(crate) async fn results(
    client: &mut Client,
    project: ProjectKey,
    run_id: Uuid,
    query_revision: u64,
    after: Option<u64>,
    limit: usize,
) -> Result<IntruderResultPage, String> {
    client
        .commands
        .send(Command::QueryIntruderResults {
            project,
            run_id,
            query_revision,
            after,
            limit,
        })
        .await
        .unwrap();
    match event(client, |e| {
        matches!(e, Event::IntruderResults {
            project: key, run_id: id, query_revision: revision, ..
        } if *key == project && *id == run_id && *revision == query_revision)
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
        let page = results(client, project, run_id, revision, None, 500)
            .await
            .unwrap();
        assert!(page.persisted, "successful fixture results must be durable");
        assert_eq!(page.run.id, run_id);
        if page.run.state.is_terminal() {
            return page;
        }
        event(
            client,
            |e| matches!(e, Event::IntruderChanged { project: key } if *key == project),
        )
        .await;
        revision += 1;
    }
}

pub(crate) async fn receive_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut data = Vec::new();
    loop {
        let read = stream.read_buf(&mut data).await.unwrap();
        assert!(read > 0 && data.len() < 8192);
        if let Some(end) = data.windows(4).position(|b| b == b"\r\n\r\n") {
            let head = std::str::from_utf8(&data[..end]).unwrap();
            let length = head
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                .expect("the fixture sends a fixed-length body");
            assert!(data.len() <= end + 4 + length);
            if data.len() == end + 4 + length {
                return data;
            }
        }
    }
}

pub(crate) fn body(raw: &[u8]) -> &[u8] {
    let end = raw.windows(4).position(|b| b == b"\r\n\r\n").unwrap();
    &raw[end + 4..]
}

pub(crate) fn header_values<'a>(raw: &'a [u8], name: &str) -> Vec<&'a str> {
    let end = raw.windows(4).position(|b| b == b"\r\n\r\n").unwrap();
    std::str::from_utf8(&raw[..end])
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
        .collect()
}

pub(crate) async fn echo_response(stream: &mut TcpStream, request: &[u8]) {
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body(request).len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    stream.write_all(body(request)).await.unwrap();
}

pub(crate) fn no_extra_connection(listener: TcpListener) {
    assert_eq!(
        listener.into_std().unwrap().accept().unwrap_err().kind(),
        ErrorKind::WouldBlock,
        "terminal runs and reopened projects must not emit another request"
    );
}

pub(crate) async fn shutdown(runtime: Runtime) {
    tokio::task::spawn_blocking(move || runtime.shutdown())
        .await
        .unwrap()
        .unwrap();
}
