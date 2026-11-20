use super::*;

pub(crate) async fn retention(
    client: &mut Client,
    project: ProjectKey,
    run: &IntruderRunSummary,
    choice: IntruderRetention,
    workspace: Workspace,
) -> IntruderRetentionReceipt {
    let operation_id = Uuid::new_v4();
    client
        .commands
        .send(Command::SetIntruderRetention {
            project,
            run_id: run.id,
            operation_id,
            expected_revision: run.revision,
            retention: choice,
            workspace: Box::new(workspace),
        })
        .await
        .unwrap();
    loop {
        match event(client).await {
            Event::IntruderRetentionChanged {
                project: owner,
                run_id,
                operation_id: operation,
                result,
            } => {
                assert_eq!((owner, run_id, operation), (project, run.id, operation_id));
                let receipt = result.unwrap();
                assert_eq!(receipt.retention, choice);
                assert_eq!(receipt.run.retention, choice);
                return receipt;
            }
            Event::Error {
                operation, message, ..
            } => panic!("{operation}: {message}"),
            _ => {}
        }
    }
}

pub(crate) async fn discard(
    client: &mut Client,
    project: ProjectKey,
    run: &IntruderRunSummary,
    operation_id: Uuid,
    workspace: Workspace,
) -> Result<IntruderDiscardReceipt, String> {
    client
        .commands
        .send(Command::DiscardIntruder {
            project,
            run_id: run.id,
            operation_id,
            expected_revision: run.revision,
            workspace: Box::new(workspace),
        })
        .await
        .unwrap();
    loop {
        match event(client).await {
            Event::IntruderDiscarded {
                project: owner,
                run_id,
                operation_id: operation,
                result,
            } => {
                assert_eq!((owner, run_id, operation), (project, run.id, operation_id));
                return result;
            }
            Event::WorkspaceChanged { .. } | Event::WorkspaceRejected { .. } => {
                panic!("discard must return its correlated receipt")
            }
            Event::Error {
                operation, message, ..
            } => panic!("{operation}: {message}"),
            _ => {}
        }
    }
}

pub(crate) async fn stopped(runtime: Runtime, client: &mut Client) {
    client.shutdown.cancel();
    loop {
        match event(client).await {
            Event::Stopped => break,
            Event::Error {
                operation, message, ..
            } => panic!("{operation}: {message}"),
            _ => {}
        }
    }
    tokio::task::spawn_blocking(move || runtime.join())
        .await
        .unwrap()
        .unwrap();
}

#[derive(Clone, Copy)]
pub(crate) enum SessionEnd {
    Shutdown,
    Promote,
    Discard,
}
