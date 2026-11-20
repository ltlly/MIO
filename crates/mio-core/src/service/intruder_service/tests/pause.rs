use super::*;
use std::future::Future;

mod cancel;
mod control;
mod recovery;
mod running;

async fn control(
    state: &mut State,
    events: &mut mpsc::Receiver<Event>,
    project: ProjectKey,
    id: Uuid,
    paused: bool,
) -> std::result::Result<IntruderRunSummary, String> {
    let operation_id = Uuid::new_v4();
    state
        .command(Command::SetIntruderPaused {
            project,
            run_id: id,
            operation_id,
            paused,
        })
        .await
        .unwrap();
    while let Ok(event) = events.try_recv() {
        if let Event::IntruderRunControl {
            project: actual_project,
            run_id,
            operation_id: actual_operation,
            result,
        } = event
        {
            assert_eq!(
                (actual_project, run_id, actual_operation),
                (project, id, operation_id)
            );
            return result;
        }
    }
    panic!("every pause/resume command must publish an identity-bound outcome");
}
