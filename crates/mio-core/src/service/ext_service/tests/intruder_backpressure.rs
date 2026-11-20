use std::{
    future::{Future, poll_fn},
    pin::Pin,
    task::Poll,
};

use super::*;

async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

#[tokio::test]
async fn cancelled_inspection_is_reclaimed_during_history_reply_wait_without_overtaking_it() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let directory = tempfile::tempdir().unwrap();
        let (source, _) = fixture(directory.path(), 72);
        let inspection = inspected_with_snapshot(source);
        let snapshot = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.is_dir())
            .unwrap();
        let (events, mut received) = mpsc::channel(2);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let project = state.project;
        let job_id = Uuid::new_v4();
        let job_cancel = CancellationToken::new();
        let (release, released) = oneshot::channel::<()>();
        state
            .ext_archive
            .start(project, job_id, job_cancel.clone(), true, move |_, _| {
                let _ = released.blocking_recv();
                Output::Inspected(Box::new(inspection))
            });
        let worker = state.ext_archive.active.as_ref().unwrap().worker.abort_handle();
        for _ in 0..2 {
            state.events.try_send(Event::Stopped).unwrap();
        }
        let (reclaimed_without_drain, reply_kept_its_slot, mut replies) = {
            let mut command = Box::pin(state.command(Command::QueryHistory {
                project,
                query_revision: 91,
                offset: 0,
                limit: 1,
                filter: String::new(),
            }));
            assert!(poll_once(command.as_mut()).await.is_pending());
            release.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(1), async {
                while !worker.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            job_cancel.cancel();
            let reclaimed = tokio::time::timeout(Duration::from_secs(1), async {
                while snapshot.exists() {
                    assert!(poll_once(command.as_mut()).await.is_pending());
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_ok();
            assert_eq!(received.len(), received.max_capacity());
            assert!(matches!(received.try_recv(), Ok(Event::Stopped)));
            let completed = tokio::time::timeout(Duration::from_secs(1), command.as_mut()).await;
            let kept_slot = completed.is_ok();
            let mut replies = Vec::new();
            while let Ok(event) = received.try_recv() {
                replies.push(event);
            }
            match completed {
                Ok(result) => result.unwrap(),
                Err(_) => {
                    tokio::time::timeout(Duration::from_secs(1), command.as_mut())
                        .await
                        .unwrap()
                        .unwrap();
                    while let Ok(event) = received.try_recv() {
                        replies.push(event);
                    }
                }
            }
            (reclaimed, kept_slot, replies)
        };
        finish(&mut state).await;
        while let Ok(event) = received.try_recv() {
            replies.push(event);
        }
        while !state.ext_archive.can_accept_commands() {
            let signal = state.ext_archive.next_signal(&state.events, false).await;
            state.ext_signal(signal).await;
            while let Ok(event) = received.try_recv() {
                replies.push(event);
            }
        }
        state.stop_ext().await.unwrap();
        assert!(!snapshot.exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(state.ext_archive.can_quiesce());
        assert!(
            reclaimed_without_drain,
            "ordinary reply waits must join and release a cancelled inspection"
        );
        assert!(
            reply_kept_its_slot,
            "the original reply must keep the first released UI slot"
        );
        let history: Vec<_> = replies.iter().enumerate().filter(|(_, event)| matches!(event,
            Event::HistoryPage { project: owner, query_revision: 91, offset: 0, total: 0, rows }
                if *owner == project && rows.is_empty()
        )).collect();
        let ext_archive: Vec<_> = replies.iter().enumerate().filter(|(_, event)| matches!(event,
            Event::ExtJob { project: owner, job_id: job, outcome: ExtJobOutcome::Cancelled }
                if *owner == project && *job == job_id
        )).collect();
        assert_eq!(history.len(), 1);
        assert_eq!(ext_archive.len(), 1);
        assert!(history[0].0 < ext_archive[0].0);
    })
    .await
    .expect("ordinary reply backpressure and owned snapshot cleanup are bounded");
}

#[tokio::test]
async fn cancelled_finished_inspection_is_reclaimed_while_intruder_reply_is_blocked() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let directory = tempfile::tempdir().unwrap();
        let (source, _) = fixture(directory.path(), 72);
        let inspection = inspected_with_snapshot(source);
        let snapshot = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.is_dir())
            .unwrap();
        let (events, mut received) = mpsc::channel(2);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let project = state.project;
        let operation_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        for _ in 0..2 {
            state.events.try_send(Event::Stopped).unwrap();
        }
        state
            .command(Command::SetIntruderPaused {
                project,
                run_id,
                operation_id,
                paused: true,
            })
            .await
            .unwrap();
        let reply_retained = !state.intruder.can_accept_commands();
        // Keep both startup events ahead of the retained reply.
        while received.try_recv().is_ok() {}

        let job_id = Uuid::new_v4();
        let job_cancel = CancellationToken::new();
        let (release, released) = oneshot::channel::<()>();
        state
            .ext_archive
            .start(project, job_id, job_cancel.clone(), true, move |_, _| {
                let _ = released.blocking_recv();
                Output::Inspected(Box::new(inspection))
            });
        let worker = state.ext_archive.active.as_ref().unwrap().worker.abort_handle();
        let shutdown = state.cancel.clone();
        let (_commands, incoming) = mpsc::channel(1);
        let mut run = Box::pin(state.run(incoming));
        let mut ended = match poll_once(run.as_mut()).await {
            Poll::Pending => None,
            Poll::Ready(result) => Some(result),
        };
        let initialized = ended.is_none();
        let initially_full = received.len() == received.max_capacity();
        let released = release.send(()).is_ok();
        let finished = tokio::time::timeout(Duration::from_secs(1), async {
            while !worker.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        job_cancel.cancel();

        // Only joining the Inspected output can start release of this owned snapshot.
        let reclaimed_without_drain = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if ended.is_some() {
                    break false;
                }
                if let Poll::Ready(result) = poll_once(run.as_mut()).await {
                    ended = Some(result);
                    break false;
                }
                if !snapshot.exists() {
                    break true;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or(false);
        let remained_full = received.len() == received.max_capacity();
        let mut replies = Vec::new();
        let delivered = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                while let Ok(event) = received.try_recv() {
                    replies.push(event);
                }
                let intruder = replies.iter().any(|event| {
                    matches!(
                        event,
                        Event::IntruderRunControl { operation_id: operation, .. }
                            if *operation == operation_id
                    )
                });
                let ext_archive = replies.iter().any(|event| {
                    matches!(
                        event,
                        Event::ExtJob { job_id: job, .. } if *job == job_id
                    )
                });
                if intruder && ext_archive {
                    break true;
                }
                if ended.is_some() {
                    break false;
                }
                if let Poll::Ready(result) = poll_once(run.as_mut()).await {
                    ended = Some(result);
                    break false;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or(false);

        shutdown.cancel();
        let stopped = match ended {
            Some(result) => result,
            None => tokio::time::timeout(Duration::from_secs(1), run.as_mut())
                .await
                .expect("the released External worker must join during fixture cleanup"),
        };
        while let Ok(event) = received.try_recv() {
            replies.push(event);
        }
        assert!(initialized && reply_retained && initially_full);
        assert!(
            released && finished,
            "the original worker finished before cancellation and observation"
        );
        stopped.unwrap();
        assert!(
            !snapshot.exists(),
            "cleanup must finish even on the red path"
        );
        assert!(
            reclaimed_without_drain,
            "Intruder reply backpressure must not suppress External join and inspection release"
        );
        assert!(
            remained_full,
            "snapshot reclamation did not rely on UI consumption"
        );
        assert!(delivered);
        assert_eq!(
            replies
                .iter()
                .filter(|event| matches!(
                    event,
                    Event::IntruderRunControl {
                        project: owner,
                        run_id: id,
                        operation_id: operation,
                        result: Err(_),
                    } if *owner == project && *id == run_id && *operation == operation_id
                ))
                .count(),
            1
        );
        assert_eq!(
            replies
                .iter()
                .filter(|event| matches!(
                    event,
                    Event::ExtJob {
                        project: owner,
                        job_id: job,
                        outcome: ExtJobOutcome::Cancelled,
                    } if *owner == project && *job == job_id
                ))
                .count(),
            1
        );
        assert_eq!(
            replies
                .iter()
                .filter(|event| matches!(
                    event,
                    Event::IntruderRunControl { operation_id: operation, .. }
                        if *operation == operation_id
                ))
                .count(),
            1
        );
        assert_eq!(
            replies
                .iter()
                .filter(|event| matches!(
                    event,
                    Event::ExtJob { job_id: job, .. } if *job == job_id
                ))
                .count(),
            1
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    })
    .await
    .expect("finished worker cancellation, snapshot release and fixture joins are bounded");
}
