use std::time::Duration;

use mio_core::{
    Client, Command, Draft, Event, FlowRecord, HttpVersion, IntruderResultOutcome,
    IntruderResultPage, IntruderRetention, IntruderRunState, IntruderRunSummary, IntruderSpec,
    IntruderTab, ProjectKey, Runtime, Uuid, Workspace,
    intruder::{AttackType, Position},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

async fn event(client: &mut Client, matches: impl Fn(&Event) -> bool) -> Event {
    loop {
        let event = client
            .events
            .recv()
            .await
            .expect("Runtime event stream remains open");
        if matches(&event) {
            return event;
        }
        if let Event::Error {
            operation, message, ..
        } = event
        {
            panic!("{operation}: {message}");
        }
    }
}

async fn opened(client: &mut Client) -> (ProjectKey, Workspace) {
    let Event::ProjectOpened {
        project, workspace, ..
    } = event(client, |event| matches!(event, Event::ProjectOpened { .. })).await
    else {
        unreachable!()
    };
    (project, workspace)
}

async fn save(client: &mut Client, project: ProjectKey, path: Option<std::path::PathBuf>) {
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

async fn results(
    client: &mut Client,
    project: ProjectKey,
    run: Uuid,
    revision: u64,
) -> IntruderResultPage {
    client
        .commands
        .send(Command::QueryIntruderResults {
            project,
            run_id: run,
            query_revision: revision,
            after: None,
            limit: 10,
        })
        .await
        .unwrap();
    let Event::IntruderResults { result, .. } = event(client, |event| matches!(event, Event::IntruderResults { project: owner, run_id, query_revision, .. } if *owner == project && *run_id == run && *query_revision == revision)).await else { unreachable!() };
    result.unwrap()
}

async fn terminal(client: &mut Client, project: ProjectKey, run: Uuid) -> IntruderResultPage {
    let mut revision = 1;
    client
        .commands
        .send(Command::QueryIntruderResults {
            project,
            run_id: run,
            query_revision: revision,
            after: None,
            limit: 10,
        })
        .await
        .unwrap();
    loop {
        match event(client, |_| true).await {
            Event::IntruderChanged { project: owner } if owner == project => {
                revision += 1;
                client
                    .commands
                    .send(Command::QueryIntruderResults {
                        project,
                        run_id: run,
                        query_revision: revision,
                        after: None,
                        limit: 10,
                    })
                    .await
                    .unwrap();
            }
            Event::IntruderResults {
                project: owner,
                run_id,
                result,
                ..
            } if owner == project && run_id == run => {
                let page = result.unwrap();
                if page.run.state.is_terminal() {
                    return page;
                }
            }
            Event::Error {
                operation, message, ..
            } => panic!("{operation}: {message}"),
            _ => {}
        }
    }
}

async fn read_flow(client: &mut Client, project: ProjectKey, id: Uuid) -> FlowRecord {
    client
        .commands
        .send(Command::ReadFlow {
            project,
            selection_revision: 100,
            flow_id: id,
        })
        .await
        .unwrap();
    let Event::FlowDetail { record, .. } = event(client, |event| matches!(event, Event::FlowDetail { project: owner, selection_revision: 100, .. } if *owner == project)).await else { unreachable!() };
    *record
}

fn no_disk_run(path: &std::path::Path, run: Uuid, flow: Uuid) {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let runs: u64 = connection
        .query_row(
            "SELECT count(*) FROM intruder_runs WHERE id=?1",
            [run.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let flows: u64 = connection
        .query_row(
            "SELECT count(*) FROM flows WHERE id=?1",
            [flow.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let messages: u64 = connection
        .query_row(
            "SELECT count(*) FROM messages WHERE flow=?1",
            [flow.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        (runs, flows, messages),
        (0, 0, 0),
        "ordinary Save and Save As must exclude the Session dataset and bodies"
    );
}

async fn retention(
    client: &mut Client,
    project: ProjectKey,
    run: &IntruderRunSummary,
    retention: IntruderRetention,
    workspace: &mut Workspace,
) -> IntruderRunSummary {
    workspace.revision += 1;
    let operation = Uuid::new_v4();
    client
        .commands
        .send(Command::SetIntruderRetention {
            project,
            run_id: run.id,
            operation_id: operation,
            expected_revision: run.revision,
            retention,
            workspace: Box::new(workspace.clone()),
        })
        .await
        .unwrap();
    let Event::IntruderRetentionChanged { result, .. } = event(client, |event| matches!(event, Event::IntruderRetentionChanged { project: owner, run_id, operation_id, .. } if *owner == project && *run_id == run.id && *operation_id == operation)).await else { unreachable!() };
    let receipt = result.unwrap();
    *workspace = receipt.workspace;
    receipt.run
}

#[tokio::test]
async fn session_start_save_as_and_explicit_promotion_preserve_one_real_response_without_replay() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                assert!(head.len() < 8192);
                head.push(stream.read_u8().await.unwrap());
            }
            assert!(head.starts_with(b"POST /session HTTP/1.1\r\n"));
            assert_eq!(stream.read_u8().await.unwrap(), 255);
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n\x00\xff").await.unwrap();
            drop(stream);
            listener
        });
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mio");
        let copy = directory.path().join("copy.mio");
        let (runtime, mut client) = Runtime::start().unwrap();
        let (project, mut workspace) = opened(&mut client).await;
        save(&mut client, project, Some(source.clone())).await;
        let raw = b"POST /session HTTP/1.1\r\nHost: fixture.invalid\r\nContent-Length: 1\r\n\r\nX".to_vec();
        let offset = raw.len() - 1;
        let spec = IntruderSpec {
            resource_pool: None,
            baseline_request: false,
            revision: 1,
            draft: Draft { revision: 1, target: format!("http://{address}"), raw, version: HttpVersion::Http11 },
            attack_type: AttackType::Sniper,
            positions: vec![Position { start: offset, end: offset + 1 }],
            payload_sets: vec![vec![vec![255]]],
            max_concurrent_requests: 1,
            request_timeout_ms: 1000,
            update_host_header: true,
            save_to_project: false,
        };
        let tab = Uuid::new_v4();
        workspace.intruder.tabs.push(IntruderTab { id: tab, title: "Session config".into(), group: None, spec: spec.clone(), last_run: None });
        workspace.intruder.active = Some(tab);
        workspace.revision += 1;
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::WorkspaceChanged { project: owner, workspace: saved } if *owner == project && saved == &workspace)).await;
        let run_id = Uuid::new_v4();
        client.commands.send(Command::StartIntruder { project, run_id, tab_id: tab, spec: Box::new(spec.clone()), cancel: CancellationToken::new() }).await.unwrap();
        let Event::IntruderStarted { result, .. } = event(&mut client, |event| matches!(event, Event::IntruderStarted { run_id: id, .. } if *id == run_id)).await else { unreachable!() };
        assert_eq!(result.unwrap().retention, IntruderRetention::Session);
        let page = terminal(&mut client, project, run_id).await;
        assert_eq!(page.run.state, IntruderRunState::Completed);
        assert_eq!((page.run.dispatched, page.run.completed, page.rows.len()), (1, 1, 1));
        assert!(!page.persisted);
        let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else { panic!("real request must produce a response") };
        let record = read_flow(&mut client, project, summary.id).await;
        assert_eq!(record.request.body, [255]);
        assert_eq!(record.response.as_ref().unwrap().body, [0, 255]);
        // A Session result is already kept; the UI closes its view without a retention replay.
        let kept = page.run.clone();
        assert_eq!(kept.retention, IntruderRetention::Session);
        save(&mut client, project, None).await;
        save(&mut client, project, Some(copy.clone())).await;
        no_disk_run(&source, run_id, summary.id);
        no_disk_run(&copy, run_id, summary.id);
        assert_eq!(read_flow(&mut client, project, summary.id).await, record);
        let promoted = retention(&mut client, project, &kept, IntruderRetention::Project, &mut workspace).await;
        assert_eq!(promoted.retention, IntruderRetention::Project);
        save(&mut client, project, None).await;
        let promoted_page = results(&mut client, project, run_id, 200).await;
        assert!(promoted_page.persisted);
        assert_eq!(promoted_page.rows, page.rows);
        client.commands.send(Command::OpenProject { path: copy }).await.unwrap();
        let (reopened, saved) = opened(&mut client).await;
        assert_eq!(reopened.id, project.id);
        assert!(reopened.generation > project.generation);
        assert_eq!(saved.intruder.tabs[0].spec, spec);
        assert_eq!(results(&mut client, reopened, run_id, 201).await.rows, page.rows);
        assert_eq!(read_flow(&mut client, reopened, summary.id).await, record);
        client.commands.send(Command::StartIntruder { project: reopened, run_id, tab_id: tab, spec: Box::new(spec), cancel: CancellationToken::new() }).await.unwrap();
        let Event::IntruderStarted { result, .. } = event(&mut client, |event| matches!(event, Event::IntruderStarted { run_id: id, .. } if *id == run_id)).await else { unreachable!() };
        assert!(result.is_err(), "promotion/reopen cannot make an old run ID admissible again");
        tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }).await.expect("one tiny loopback request plus Save/SaveAs/promotion/reopen and Runtime join is bounded");
}

#[tokio::test]
async fn running_session_save_ack_precedes_response_and_same_run_reopens_complete() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, seen) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                assert!(head.len() < 8192);
                head.push(stream.read_u8().await.unwrap());
            }
            assert!(head.starts_with(b"POST /live-save HTTP/1.1\r\n"));
            assert_eq!(stream.read_u8().await.unwrap(), 255);
            seen_tx.send(()).unwrap();
            released.await.unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n\x00\xff").await.unwrap();
            listener
        });
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("live-save.mio");
        let (runtime, mut client) = Runtime::start().unwrap();
        let (project, mut workspace) = opened(&mut client).await;
        save(&mut client, project, Some(path.clone())).await;
        let raw = b"POST /live-save HTTP/1.1\r\nHost: fixture.invalid\r\nContent-Length: 1\r\n\r\nX".to_vec();
        let offset = raw.len() - 1;
        let spec = IntruderSpec {
            resource_pool: None,
            baseline_request: false,
            revision: 1,
            draft: Draft { revision: 1, target: format!("http://{address}"), raw, version: HttpVersion::Http11 },
            attack_type: AttackType::Sniper,
            positions: vec![Position { start: offset, end: offset + 1 }],
            payload_sets: vec![vec![vec![255]]],
            max_concurrent_requests: 1,
            request_timeout_ms: 5000,
            update_host_header: true,
            save_to_project: false,
        };
        let tab = Uuid::new_v4();
        workspace.intruder.tabs.push(IntruderTab {
            id: tab, title: "配置名称保持".into(), group: None, spec: spec.clone(), last_run: None,
        });
        workspace.intruder.active = Some(tab);
        workspace.revision += 1;
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::WorkspaceChanged { project: owner, workspace: saved } if *owner == project && saved == &workspace)).await;
        let run_id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        client.commands.send(Command::StartIntruder {
            project, run_id, tab_id: tab, spec: Box::new(spec.clone()), cancel: cancel.clone(),
        }).await.unwrap();
        let Event::IntruderStarted { result, .. } = event(&mut client, |event| matches!(event, Event::IntruderStarted { run_id: id, .. } if *id == run_id)).await else { unreachable!() };
        assert_eq!(result.unwrap().retention, IntruderRetention::Session);
        seen.await.unwrap();
        let before = results(&mut client, project, run_id, 310).await;
        assert_eq!(before.run.state, IntruderRunState::Running);
        assert_eq!((before.run.dispatched, before.run.completed, before.rows.len()), (1, 0, 1));
        assert!(!before.persisted);
        let IntruderResultOutcome::Flow(flow) = &before.rows[0].outcome else { panic!("the pending socket must retain its flow identity") };
        let promoted = retention(&mut client, project, &before.run, IntruderRetention::Project, &mut workspace).await;
        let mut expected = before.run.clone();
        expected.revision += 1;
        expected.retention = IntruderRetention::Project;
        assert_eq!(promoted, expected);
        assert!(!cancel.is_cancelled());
        save(&mut client, project, None).await;
        let pending = results(&mut client, project, run_id, 311).await;
        assert_eq!(pending.run, promoted);
        assert_eq!(pending.rows, before.rows);
        assert!(pending.persisted);
        assert!(read_flow(&mut client, project, flow.id).await.response.is_none());
        {
            let disk = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            let pending: (u64, u64) = disk.query_row("SELECT dispatched,completed FROM intruder_runs WHERE id=?1", [run_id.to_string()], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
            assert_eq!(pending, (1, 0), "the saved attack must still be running before the origin releases its response");
        }
        release.send(()).unwrap();
        let complete = terminal(&mut client, project, run_id).await;
        assert_eq!(complete.run.state, IntruderRunState::Completed);
        assert_eq!(complete.run.id, run_id);
        assert_eq!((complete.run.dispatched, complete.run.completed), (1, 1));
        assert!(complete.persisted);
        let record = read_flow(&mut client, project, flow.id).await;
        assert_eq!(record.response.as_ref().unwrap().body, [0, 255]);
        assert!(!cancel.is_cancelled());
        save(&mut client, project, None).await;
        client.commands.send(Command::OpenProject { path }).await.unwrap();
        let (reopened, saved) = opened(&mut client).await;
        assert_eq!(saved.intruder.tabs[0].title, "配置名称保持");
        assert_eq!(saved.intruder.tabs[0].spec, spec);
        assert_eq!(results(&mut client, reopened, run_id, 312).await.rows, complete.rows);
        assert_eq!(read_flow(&mut client, reopened, flow.id).await, record);
        tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }).await.expect("live save must acknowledge before releasing one tiny response and join after explicit shutdown");
}
