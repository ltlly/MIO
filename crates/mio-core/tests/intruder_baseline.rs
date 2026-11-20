use std::{io::ErrorKind, path::PathBuf, time::Duration};

use mio_core::{
    Client, Command, Draft, Event, FlowRecord, HttpVersion, IntruderBaseline,
    IntruderBaselineReceipt, IntruderResultOutcome, IntruderResultPage, IntruderRunState,
    IntruderSpec, IntruderTab, ProjectKey, Runtime, Uuid, Workspace,
    intruder::{AttackType, Position},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

const DEADLINE: Duration = Duration::from_secs(8);

async fn event(client: &mut Client, matches: impl Fn(&Event) -> bool) -> Event {
    loop {
        let event = client
            .events
            .recv()
            .await
            .expect("Runtime remains connected");
        assert!(
            !matches!(event, Event::Error { .. }),
            "unexpected Runtime error: {event:?}"
        );
        if matches(&event) {
            return event;
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

async fn save(client: &mut Client, project: ProjectKey, path: Option<PathBuf>) {
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

fn spec(target: String, initial_baseline: bool) -> IntruderSpec {
    let raw =
        b"POST /baseline HTTP/1.1\r\nHost: fixture.invalid\r\nContent-Length: 1\r\n\r\nX".to_vec();
    let position = Position {
        start: raw.len() - 1,
        end: raw.len(),
    };
    IntruderSpec {
        resource_pool: None,
        revision: 1,
        draft: Draft {
            revision: 1,
            target,
            raw,
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![position],
        payload_sets: vec![vec![vec![0], vec![255]]],
        max_concurrent_requests: 1,
        request_timeout_ms: 1000,
        update_host_header: true,
        baseline_request: initial_baseline,
        save_to_project: true,
    }
}

async fn start(
    client: &mut Client,
    project: ProjectKey,
    workspace: &mut Workspace,
    spec: &IntruderSpec,
    cancel: CancellationToken,
) -> Uuid {
    let tab = Uuid::new_v4();
    workspace.revision += 1;
    workspace.intruder.tabs.push(IntruderTab {
        id: tab,
        title: "Baseline fixture".into(),
        group: None,
        spec: spec.clone(),
        last_run: None,
    });
    workspace.intruder.active = Some(tab);
    client
        .commands
        .send(Command::SaveWorkspace {
            project,
            workspace: workspace.clone(),
        })
        .await
        .unwrap();
    event(client, |event| matches!(event, Event::WorkspaceChanged { project: owner, workspace: saved } if *owner == project && saved == &*workspace)).await;
    let run = Uuid::new_v4();
    client
        .commands
        .send(Command::StartIntruder {
            project,
            run_id: run,
            tab_id: tab,
            spec: Box::new(spec.clone()),
            cancel,
        })
        .await
        .unwrap();
    let Event::IntruderStarted { result, .. } = event(client, |event| matches!(event, Event::IntruderStarted { project: owner, run_id, .. } if *owner == project && *run_id == run)).await else { unreachable!() };
    let summary = result.unwrap();
    assert_eq!(summary.total, 2 + u64::from(spec.baseline_request));
    assert_eq!(
        summary.baseline,
        spec.baseline_request.then_some(IntruderBaseline {
            ordinal: 0,
            completed: false
        })
    );
    run
}

async fn query(client: &Client, project: ProjectKey, run: Uuid, revision: u64) {
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

async fn results(
    client: &mut Client,
    project: ProjectKey,
    run: Uuid,
    revision: u64,
) -> IntruderResultPage {
    query(client, project, run, revision).await;
    let Event::IntruderResults { result, .. } = event(client, |event| matches!(event, Event::IntruderResults { project: owner, run_id, query_revision, .. } if *owner == project && *run_id == run && *query_revision == revision)).await else { unreachable!() };
    result.unwrap()
}

async fn terminal(
    client: &mut Client,
    project: ProjectKey,
    run: Uuid,
    mut revision: u64,
) -> IntruderResultPage {
    let minimum_revision = revision;
    query(client, project, run, revision).await;
    loop {
        match event(client, |_| true).await {
            Event::IntruderChanged { project: owner } if owner == project => {
                revision += 1;
                query(client, project, run, revision).await;
            }
            Event::IntruderResults {
                project: owner,
                run_id,
                query_revision,
                result,
            } if owner == project
                && run_id == run
                && (minimum_revision..=revision).contains(&query_revision) =>
            {
                let page = result.unwrap();
                if page.run.state.is_terminal() {
                    return page;
                }
            }
            _ => {}
        }
    }
}

async fn records(
    client: &mut Client,
    project: ProjectKey,
    page: &IntruderResultPage,
) -> Vec<FlowRecord> {
    let mut records = Vec::new();
    for row in &page.rows {
        let IntruderResultOutcome::Flow(summary) = &row.outcome else {
            panic!("every fixture row must contain a real response")
        };
        let selection_revision = 1000 + row.ordinal;
        client
            .commands
            .send(Command::ReadFlow {
                project,
                selection_revision,
                flow_id: summary.id,
            })
            .await
            .unwrap();
        let Event::FlowDetail { record, .. } = event(client, |event| matches!(event, Event::FlowDetail { project: owner, selection_revision: revision, .. } if *owner == project && *revision == selection_revision)).await else { unreachable!() };
        assert_eq!(record.summary.id, summary.id);
        records.push(*record);
    }
    records
}

async fn baseline_receipt(
    client: &mut Client,
    project: ProjectKey,
    run: Uuid,
    operation: Uuid,
) -> IntruderBaselineReceipt {
    let Event::IntruderBaselineRequested { result, .. } = event(client, |event| matches!(event, Event::IntruderBaselineRequested { project: owner, run_id, operation_id, .. } if *owner == project && *run_id == run && *operation_id == operation)).await else { unreachable!() };
    result.unwrap()
}

async fn origin(listener: TcpListener, expected: Vec<u8>) -> TcpListener {
    for expected in expected {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            assert!(head.len() < 8192);
            head.push(stream.read_u8().await.unwrap());
        }
        assert!(head.starts_with(b"POST /baseline HTTP/1.1\r\n"));
        assert_eq!(stream.read_u8().await.unwrap(), expected);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        stream.write_all(&[0, expected]).await.unwrap();
    }
    listener
}

async fn shutdown(runtime: Runtime, listener: TcpListener) {
    tokio::task::spawn_blocking(move || runtime.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        listener.into_std().unwrap().accept().unwrap_err().kind(),
        ErrorKind::WouldBlock,
        "no payload replay or extra baseline socket"
    );
}

#[tokio::test]
async fn completed_baseline_ack_backpressure_allows_queued_stop_without_another_socket() {
    tokio::time::timeout(DEADLINE, async {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let path = directory.path().join("backpressure.mio");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let input = spec(format!("http://{}", listener.local_addr().unwrap()), false);
        let origin = tokio::spawn(origin(listener, vec![0, 255]));
        let (runtime, mut client) = Runtime::start().unwrap();
        let (project, mut workspace) = opened(&mut client).await;
        save(&mut client, project, Some(path.clone())).await;
        let old_cancel = CancellationToken::new();
        let run = start(&mut client, project, &mut workspace, &input, old_cancel.clone()).await;
        let before = terminal(&mut client, project, run, 1).await;
        assert_eq!(before.run.state, IntruderRunState::Completed);
        assert_eq!((before.run.dispatched, before.run.completed, before.rows.len()), (2, 2, 2));
        let before_records = records(&mut client, project, &before).await;
        let listener = origin.await.unwrap();
        old_cancel.cancel();

        // Synchronize and clear the terminal run's history/Intruder notifications first.
        client.commands.send(Command::QueryHistory { project, query_revision: 500, offset: 0, limit: 1, filter: String::new() }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::HistoryPage { project: owner, query_revision: 500, .. } if *owner == project)).await;
        while let Ok(event) = client.events.try_recv() {
            assert!(!matches!(event, Event::Error { .. }), "unexpected event: {event:?}");
        }
        let capacity = client.events.max_capacity();
        for query_revision in 0..capacity as u64 {
            client.commands.send(Command::QueryIntruderRuns { project, query_revision, after: None, limit: 1 }).await.unwrap();
        }
        while client.events.len() != capacity {
            tokio::task::yield_now().await;
        }
        let operation = Uuid::new_v4();
        let admission = CancellationToken::new();
        client.commands.send(Command::RequestIntruderBaseline {
            project, run_id: run, operation_id: operation,
            expected_revision: before.run.revision, cancel: admission.clone(),
        }).await.unwrap();
        let disk = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let phase = || disk.query_row(
            "SELECT state,total,dispatched,completed,baseline_ordinal FROM intruder_runs WHERE id=?1",
            [run.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, u64>(1)?, row.get::<_, u64>(2)?, row.get::<_, u64>(3)?, row.get::<_, Option<u64>>(4)?)),
        ).unwrap();
        while phase() != (0, 3, 2, 2, Some(2)) {
            tokio::task::yield_now().await;
        }
        assert_eq!(client.events.len(), capacity, "accepted receipt remains backpressured");
        admission.cancel();
        client.commands.send(Command::StopIntruder { project, run_id: run }).await.unwrap();

        for expected_revision in 0..capacity as u64 {
            let Event::IntruderRuns { project: owner, query_revision, result } = client.events.recv().await.unwrap() else { panic!("only the controlled filler responses precede the retained receipt") };
            assert_eq!((owner, query_revision), (project, expected_revision));
            assert_eq!(result.unwrap().rows[0], before.run);
            if expected_revision == 0 {
                // The retained ACK occupies this one released slot; no dispatch slot is free.
                while client.events.len() != capacity || phase() != (2, 3, 2, 2, Some(2)) {
                    tokio::task::yield_now().await;
                }
            }
        }
        let receipt = baseline_receipt(&mut client, project, run, operation).await;
        assert_eq!(receipt.run.state, IntruderRunState::Running);
        assert_eq!(receipt.run.revision, before.run.revision + 1);
        assert_eq!(receipt.run.baseline, Some(IntruderBaseline { ordinal: 2, completed: false }));
        assert!(receipt.cancel.is_cancelled(), "queued Stop must cancel the newly returned Runtime token");
        let stopped = terminal(&mut client, project, run, 2000).await;
        assert_eq!(stopped.run.state, IntruderRunState::Stopped);
        assert_eq!(stopped.run.payload_progress(), Some((2, 2)));
        assert_eq!(stopped.run.baseline, Some(IntruderBaseline { ordinal: 2, completed: false }));
        assert_eq!(stopped.rows, before.rows);
        assert_eq!(records(&mut client, project, &stopped).await, before_records);
        shutdown(runtime, listener).await;
    }).await.expect("baseline ACK backpressure, queued Stop, unchanged rows and normal join are bounded");
}

async fn assert_baseline_persists(initial: bool) {
    tokio::time::timeout(DEADLINE, async {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let source = directory.path().join("source.mio");
        let copy = directory.path().join("copy.mio");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let input = spec(format!("http://{}", listener.local_addr().unwrap()), initial);
        let expected = if initial { vec![b'X', 0, 255] } else { vec![0, 255, b'X'] };
        let origin = tokio::spawn(origin(listener, expected.clone()));
        let (runtime, mut client) = Runtime::start().unwrap();
        let (project, mut workspace) = opened(&mut client).await;
        save(&mut client, project, Some(source.clone())).await;
        let old_cancel = CancellationToken::new();
        let run = start(&mut client, project, &mut workspace, &input, old_cancel.clone()).await;
        let first = terminal(&mut client, project, run, 1).await;
        assert_eq!(first.run.state, IntruderRunState::Completed);
        let final_page = if initial {
            first
        } else {
            assert_eq!(first.run.baseline, None);
            assert_eq!((first.run.total, first.rows.len()), (2, 2));
            old_cancel.cancel();
            let operation = Uuid::new_v4();
            let admission = CancellationToken::new();
            client.commands.send(Command::RequestIntruderBaseline {
                project, run_id: run, operation_id: operation,
                expected_revision: first.run.revision, cancel: admission.clone(),
            }).await.unwrap();
            let receipt = baseline_receipt(&mut client, project, run, operation).await;
            admission.cancel();
            assert!(!receipt.cancel.is_cancelled(), "old and admission tokens do not cancel the accepted baseline");
            assert_eq!(receipt.run.id, run);
            assert_eq!((receipt.run.total, receipt.run.dispatched, receipt.run.completed), (3, 2, 2));
            let completed = terminal(&mut client, project, run, 100).await;
            assert_eq!(completed.rows[..2], first.rows);
            assert!(!receipt.cancel.is_cancelled());
            completed
        };
        assert_eq!(final_page.run.state, IntruderRunState::Completed);
        assert_eq!((final_page.run.total, final_page.run.dispatched, final_page.run.completed), (3, 3, 3));
        assert_eq!(final_page.run.payload_progress(), Some((2, 2)));
        let baseline_ordinal = if initial { 0 } else { 2 };
        assert_eq!(final_page.run.baseline, Some(IntruderBaseline { ordinal: baseline_ordinal, completed: true }));
        assert_eq!(final_page.run.request_number(baseline_ordinal), Some(0));
        assert!(final_page.persisted);
        let saved_records = records(&mut client, project, &final_page).await;
        assert_eq!(saved_records.len(), expected.len());
        for (record, expected) in saved_records.iter().zip(expected) {
            assert_eq!(record.request.body, [expected]);
            assert_eq!(record.response.as_ref().unwrap().body, [0, expected]);
        }
        save(&mut client, project, None).await;
        save(&mut client, project, Some(copy.clone())).await;
        let mut generation = project.generation;
        for path in [source, copy] {
            client.commands.send(Command::OpenProject { path }).await.unwrap();
            let (reopened, saved) = opened(&mut client).await;
            assert_eq!(reopened.id, project.id);
            assert!(reopened.generation > generation);
            generation = reopened.generation;
            assert_eq!(saved.intruder.tabs[0].spec, input, "late baseline does not rewrite the frozen startup preference");
            client.commands.send(Command::ReadIntruderSpec { project: reopened, run_id: run, selection_revision: generation }).await.unwrap();
            let Event::IntruderSpec { result, .. } = event(&mut client, |event| matches!(event, Event::IntruderSpec { project: owner, run_id, selection_revision, .. } if *owner == reopened && *run_id == run && *selection_revision == generation)).await else { unreachable!() };
            assert_eq!(*result.unwrap(), input);
            let page = results(&mut client, reopened, run, 3000 + generation).await;
            assert_eq!(page, final_page);
            assert_eq!(records(&mut client, reopened, &page).await, saved_records);
        }
        shutdown(runtime, origin.await.unwrap()).await;
    }).await.expect("two payloads plus one baseline survive Save, SaveAs, both reopens and normal join");
}

#[tokio::test]
async fn initial_baseline_save_save_as_and_reopen_preserve_original_and_payload_results() {
    assert_baseline_persists(true).await;
}

#[tokio::test]
async fn late_completed_baseline_save_save_as_and_reopen_preserve_existing_payload_results() {
    assert_baseline_persists(false).await;
}
