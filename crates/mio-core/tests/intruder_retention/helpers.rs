pub(crate) use std::{io::ErrorKind, path::PathBuf, time::Duration};

pub(crate) use mio_core::{
    intruder::{AttackType, Position},
    *,
};
pub(crate) use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
pub(crate) use tokio_util::sync::CancellationToken;

pub(crate) async fn event(client: &mut Client) -> Event {
    client
        .events
        .recv()
        .await
        .expect("runtime event channel closed before acknowledgement")
}

pub(crate) async fn opened(client: &mut Client) -> (ProjectKey, Workspace) {
    loop {
        match event(client).await {
            Event::ProjectOpened {
                project, workspace, ..
            } => return (project, workspace),
            Event::Error {
                operation, message, ..
            } => panic!("{operation}: {message}"),
            _ => {}
        }
    }
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
    loop {
        match event(client).await {
            Event::WorkspaceChanged {
                project: owner,
                workspace: stored,
            } => {
                assert_eq!(owner, project);
                assert_eq!(&stored, workspace);
                return;
            }
            Event::WorkspaceRejected { message, .. } | Event::Error { message, .. } => {
                panic!("{message}")
            }
            _ => {}
        }
    }
}

pub(crate) async fn save(client: &mut Client, project: ProjectKey, path: PathBuf) {
    client
        .commands
        .send(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project,
            path: Some(path.clone()),
            workspace: None,
        })
        .await
        .unwrap();
    loop {
        match event(client).await {
            Event::ProjectSaved {
                project: owner,
                path: saved,
                total_bytes,
                ..
            } => {
                assert_eq!(
                    owner, project,
                    "SaveAs must not change the Session generation or UUID"
                );
                assert_eq!(saved, path);
                assert!(total_bytes > 0);
                return;
            }
            Event::Error {
                operation, message, ..
            } => panic!("{operation}: {message}"),
            _ => {}
        }
    }
}

pub(crate) async fn query_results(
    client: &mut Client,
    project: ProjectKey,
    run_id: Uuid,
    revision: u64,
) {
    client
        .commands
        .send(Command::QueryIntruderResults {
            project,
            run_id,
            query_revision: revision,
            after: None,
            limit: 10,
        })
        .await
        .unwrap();
}

pub(crate) async fn terminal_results(
    client: &mut Client,
    project: ProjectKey,
    run_id: Uuid,
) -> IntruderResultPage {
    let mut revision = 1;
    query_results(client, project, run_id, revision).await;
    loop {
        match event(client).await {
            Event::IntruderChanged { project: owner } if owner == project => {
                revision += 1;
                query_results(client, project, run_id, revision).await;
            }
            Event::IntruderResults {
                project: owner,
                run_id: id,
                result,
                ..
            } if owner == project && id == run_id => {
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

pub(crate) async fn results(
    client: &mut Client,
    project: ProjectKey,
    run_id: Uuid,
) -> IntruderResultPage {
    query_results(client, project, run_id, 100).await;
    loop {
        match event(client).await {
            Event::IntruderResults {
                project: owner,
                run_id: id,
                query_revision: 100,
                result,
            } => {
                assert_eq!((owner, id), (project, run_id));
                return result.unwrap();
            }
            Event::Error {
                operation, message, ..
            } => panic!("{operation}: {message}"),
            _ => {}
        }
    }
}

pub(crate) async fn runs(
    client: &mut Client,
    project: ProjectKey,
    revision: u64,
) -> IntruderRunPage {
    client
        .commands
        .send(Command::QueryIntruderRuns {
            project,
            query_revision: revision,
            after: None,
            limit: 10,
        })
        .await
        .unwrap();
    loop {
        match event(client).await {
            Event::IntruderRuns {
                project: owner,
                query_revision,
                result,
            } if query_revision == revision => {
                assert_eq!(owner, project);
                return result.unwrap();
            }
            Event::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
}

pub(crate) async fn missing(
    client: &mut Client,
    project: ProjectKey,
    run_id: Uuid,
    records: &[FlowRecord],
) {
    let page = runs(client, project, 600).await;
    assert!(
        page.rows.is_empty(),
        "discarded Session must not remain in the run list"
    );
    assert_eq!(page.next_after, None);
    query_results(client, project, run_id, 601).await;
    loop {
        match event(client).await {
            Event::IntruderResults {
                project: owner,
                run_id: id,
                query_revision: 601,
                result,
            } => {
                assert_eq!((owner, id), (project, run_id));
                assert!(
                    result.is_err(),
                    "discarded Session results must be unavailable"
                );
                break;
            }
            Event::Error {
                operation, message, ..
            } => panic!("{operation}: {message}"),
            _ => {}
        }
    }
    client
        .commands
        .send(Command::ReadIntruderSpec {
            project,
            run_id,
            selection_revision: 602,
        })
        .await
        .unwrap();
    loop {
        match event(client).await {
            Event::IntruderSpec {
                project: owner,
                run_id: id,
                selection_revision: 602,
                result,
            } => {
                assert_eq!((owner, id), (project, run_id));
                assert!(
                    result.is_err(),
                    "discarded frozen inputs must be unavailable"
                );
                break;
            }
            Event::Error {
                operation, message, ..
            } => panic!("{operation}: {message}"),
            _ => {}
        }
    }
    for (index, record) in records.iter().enumerate() {
        let revision = 603 + index as u64;
        client
            .commands
            .send(Command::ReadFlow {
                project,
                flow_id: record.summary.id,
                selection_revision: revision,
            })
            .await
            .unwrap();
        loop {
            match event(client).await {
                Event::FlowDetailError {
                    project: owner,
                    selection_revision,
                    flow_id,
                    ..
                } if selection_revision == revision => {
                    assert_eq!((owner, flow_id), (project, record.summary.id));
                    break;
                }
                Event::FlowDetail {
                    selection_revision, ..
                } if selection_revision == revision => panic!("discarded Session body survived"),
                Event::Error {
                    operation, message, ..
                } => panic!("{operation}: {message}"),
                _ => {}
            }
        }
    }
}

pub(crate) async fn read_flow(
    client: &mut Client,
    project: ProjectKey,
    flow_id: FlowId,
) -> FlowRecord {
    client
        .commands
        .send(Command::ReadFlow {
            project,
            flow_id,
            selection_revision: 200,
        })
        .await
        .unwrap();
    loop {
        match event(client).await {
            Event::FlowDetail {
                project: owner,
                selection_revision: 200,
                record,
                ..
            } => {
                assert_eq!(owner, project);
                assert_eq!(record.summary.id, flow_id);
                return *record;
            }
            Event::FlowDetailError { message, .. } | Event::Error { message, .. } => {
                panic!("{message}")
            }
            _ => {}
        }
    }
}

pub(crate) async fn read_spec(
    client: &mut Client,
    project: ProjectKey,
    run_id: Uuid,
) -> IntruderSpec {
    client
        .commands
        .send(Command::ReadIntruderSpec {
            project,
            run_id,
            selection_revision: 300,
        })
        .await
        .unwrap();
    loop {
        match event(client).await {
            Event::IntruderSpec {
                project: owner,
                run_id: id,
                selection_revision: 300,
                result,
            } => {
                assert_eq!((owner, id), (project, run_id));
                return *result.unwrap();
            }
            Event::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
}
