use std::{
    future::{Future, poll_fn},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

use tokio::task::AbortHandle;

use super::*;

const BOUND: Duration = Duration::from_secs(1);

async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

async fn workers_finished(workers: &[AbortHandle]) -> bool {
    tokio::time::timeout(BOUND, async {
        while workers.iter().any(|worker| !worker.is_finished()) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_ok()
}

fn held_cancelled_completion(
    state: &mut State,
    run_id: Uuid,
    ordinal: u64,
) -> (AbortHandle, oneshot::Sender<()>) {
    let mut record = crate::service::tests::completed_flow();
    record.summary.source = TrafficSource::Intruder;
    record.summary.revision = 1;
    record.summary.state = FlowState::Pending;
    record.summary.status = None;
    record.summary.response_bytes = 0;
    record.response = None;
    state
        .persist_intruder_result(run_id, ordinal, Ok(Box::new(record.clone())))
        .unwrap();
    let project = state.project;
    let flow_id = record.summary.id;
    let (release, released) = oneshot::channel();
    let worker = state.intruder.jobs.spawn(async move {
        // Hold only the completion tail, as in the existing ownership/deadline fixture.
        let _ = released.await;
        record.summary.revision += 1;
        record.summary.state = FlowState::Cancelled;
        Completion {
            project,
            record,
            reason_phrase: None,
        }
    });
    state.intruder.requests.insert(
        worker.id(),
        ActiveRequest {
            run_id,
            ordinal,
            flow_id,
            resource_pool: None,
        },
    );
    state.intruder.runs.get_mut(&run_id).unwrap().active += 1;
    (worker, release)
}

#[tokio::test]
async fn automatic_resource_cleanup_preserves_the_first_control_write_error() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let (mut state, mut events) = state();
        let project = state.project;
        let id = admit(&mut state, spec("http://127.0.0.1:1".into(), 1));
        state.set_intruder_paused(project, id, true).unwrap();
        state.store.reject_writes(true);
        let first_error = state
            .store
            .set_intruder_state(id, IntruderRunState::Running)
            .unwrap_err()
            .to_string();
        let shutdown = state.cancel.clone();
        let (commands, incoming) = mpsc::channel(1);
        let mut run = Box::pin(state.run(incoming));
        assert!(poll_once(run.as_mut()).await.is_pending());
        while events.try_recv().is_ok() {}
        let operation_id = Uuid::new_v4();
        commands
            .try_send(Command::SetIntruderPaused {
                project,
                run_id: id,
                operation_id,
                paused: false,
            })
            .unwrap();
        assert!(poll_once(run.as_mut()).await.is_pending());
        let mut errors = Vec::new();
        let mut rejected = false;
        while let Ok(event) = events.try_recv() {
            match event {
                Event::StorageState {
                    project: owner,
                    pending_records,
                    message,
                } if owner == project && pending_records > 0 => errors.push(message),
                Event::IntruderRunControl {
                    operation_id: operation,
                    result: Err(_),
                    ..
                } if operation == operation_id => rejected = true,
                _ => {}
            }
        }
        shutdown.cancel();
        let stopped = tokio::time::timeout(BOUND, run.as_mut()).await.unwrap();
        assert!(stopped.unwrap_err().to_string().contains("未能写盘"));
        assert!(rejected && !errors.is_empty());
        assert!(
            errors
                .iter()
                .all(|error| error.as_ref() == Some(&first_error)),
            "automatic cleanup must retain the original write failure: {errors:?}"
        );
    })
    .await
    .expect("failed Resume and its automatic cleanup have no admitted sockets");
}

#[tokio::test]
async fn ready_resume_commands_cannot_overtake_idle_intruder_cancellation() {
    assert_resume_commands_yield_to_reclamation(false).await;
}

#[tokio::test]
async fn ready_resume_commands_cannot_overtake_intruder_completion_and_terminal_state() {
    assert_resume_commands_yield_to_reclamation(true).await;
}

async fn assert_resume_commands_yield_to_reclamation(with_completion: bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        let (mut state, mut events) = state();
        let project = state.project;
        let id = admit(&mut state, spec("http://127.0.0.1:1".into(), 2));
        let mut workers = Vec::new();
        let mut releases = Vec::new();
        if with_completion {
            let (worker, release) = held_cancelled_completion(&mut state, id, 0);
            workers.push(worker);
            releases.push(release);
        }
        state.set_intruder_paused(project, id, true).unwrap();
        state.intruder_run_page(None, 1).unwrap();
        let run_cancel = state.intruder.runs[&id].cancel.clone();
        let shutdown = state.cancel.clone();
        let (commands, incoming) = mpsc::channel(9);
        let mut run = Box::pin(state.run(incoming));
        let mut ended = match poll_once(run.as_mut()).await {
            Poll::Pending => None,
            Poll::Ready(result) => Some(result),
        };
        let initialized = ended.is_none();
        while events.try_recv().is_ok() {}

        run_cancel.cancel();
        let mut released = true;
        for release in releases {
            released &= release.send(()).is_ok();
        }
        let finished = workers_finished(&workers).await;
        for _ in 0..8 {
            commands.try_send(Command::Resume { project }).unwrap();
        }
        // The page captures the state before any later emit or shutdown can repair it.
        commands
            .try_send(Command::QueryIntruderResults {
                project,
                run_id: id,
                query_revision: 71,
                after: None,
                limit: 2,
            })
            .unwrap();
        if ended.is_none()
            && let Poll::Ready(result) = poll_once(run.as_mut()).await
        {
            ended = Some(result);
        }
        let mut page = None;
        while let Ok(event) = events.try_recv() {
            if let Event::IntruderResults {
                project: owner,
                run_id,
                query_revision: 71,
                result,
            } = event
                && owner == project
                && run_id == id
            {
                page = Some(result);
            }
        }

        shutdown.cancel();
        let stopped = match ended {
            Some(result) => result,
            None => tokio::time::timeout(BOUND, run.as_mut())
                .await
                .expect("ready Intruder senders must join during fixture cleanup"),
        };
        assert!(initialized);
        assert!(
            released && finished,
            "the completion was ready before Resume admission"
        );
        stopped.unwrap();
        let page = page
            .expect("the finite ready command batch includes its observer")
            .unwrap();
        assert_eq!(
            page.run.state,
            IntruderRunState::Stopped,
            "cancellation, join and terminal persistence must precede already-ready Resume commands"
        );
        let count = u64::from(with_completion);
        assert!(page.persisted);
        assert_eq!((page.run.dispatched, page.run.completed), (count, count));
        assert_eq!(page.rows.len(), usize::from(with_completion));
        if with_completion {
            assert!(matches!(
                &page.rows[0].outcome,
                IntruderResultOutcome::Flow(flow) if flow.state == FlowState::Cancelled
            ));
        }
        assert!(workers.iter().all(AbortHandle::is_finished));
    })
    .await
    .expect("finite ready commands and causally released completion cleanup are bounded");
}

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn full_ui_queue_without_local_notifications_blocks_dispatch_and_capacity_wakes_it() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (mut state, mut events) = state();
        let id = admit(
            &mut state,
            spec(format!("http://{}", listener.local_addr().unwrap()), 1),
        );
        state.intruder_history_changed();
        let initial = state.intruder.next_signal(&state.events, false).await;
        assert!(matches!(initial, Signal::Notify(_)));
        state.intruder_signal(initial).await.unwrap();
        state.intruder_run_page(None, 1).unwrap();
        while events.try_recv().is_ok() {}
        for _ in 0..state.events.max_capacity() {
            state.events.try_send(Event::Stopped).unwrap();
        }
        let empty_notifications = state.intruder.can_accept_commands();
        let wake = Arc::new(WakeCounter::default());
        let waker = Waker::from(wake.clone());
        let mut context = Context::from_waker(&waker);
        let (premature, capacity_woke, released_slot, signal) = {
            let mut waiting = Box::pin(state.intruder.next_signal(&state.events, false));
            let mut ready = match waiting.as_mut().poll(&mut context) {
                Poll::Pending => None,
                Poll::Ready(signal) => Some(signal),
            };
            tokio::task::yield_now().await;
            if ready.is_none()
                && let Poll::Ready(signal) = waiting.as_mut().poll(&mut context)
            {
                ready = Some(signal);
            }
            tokio::task::yield_now().await;
            let premature = ready.is_some();
            wake.0.store(0, Ordering::SeqCst);
            let released_slot = matches!(events.try_recv(), Ok(Event::Stopped));
            let capacity_woke = wake.0.load(Ordering::SeqCst) != 0;
            let signal = match ready {
                Some(signal) => Some(signal),
                None => tokio::time::timeout(BOUND, waiting.as_mut()).await.ok(),
            };
            (premature, capacity_woke, released_slot, signal)
        };

        let mut origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let payload = request(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            (listener, payload)
        });
        let advanced = match signal {
            Some(signal) => Some(state.intruder_signal(signal).await),
            None => None,
        };
        let origin_result = match tokio::time::timeout(BOUND, &mut origin).await {
            Ok(result) => Some(result),
            Err(_) => {
                origin.abort();
                let _ = origin.await;
                None
            }
        };
        let stopped = state.stop_intruder().await;
        let joined = state.finish_intruder_shutdown().await;

        assert!(empty_notifications && released_slot);
        assert!(
            !premature,
            "a full UI queue cannot yield a new dispatch even after a scheduler yield"
        );
        assert!(
            capacity_woke,
            "freeing a slot must wake the registered scheduler without another command"
        );
        advanced
            .expect("capacity alone must make dispatch ready")
            .unwrap();
        stopped.unwrap();
        joined.unwrap();
        let (listener, payload) = origin_result
            .expect("one real request reaches the origin")
            .unwrap();
        assert_eq!(payload, 0xff);
        assert!(!state.intruder.has_resources());
        assert!(state.intruder.requests.is_empty());
        assert_eq!(
            listener.into_std().unwrap().accept().unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
        assert_eq!(state.intruder_run(id).unwrap().dispatched, 1);
    })
    .await
    .expect("capacity wakeup, one loopback request and all fixture joins are bounded");
}

#[tokio::test]
async fn eight_runs_sixteen_completions_retain_all_write_faults_within_34_notifications() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut state, mut events) = state();
        let project = state.project;
        let mut ids = Vec::new();
        let mut workers = Vec::new();
        let mut releases = Vec::new();
        const PER_RUN_COMPLETIONS: usize = MAX_REQUESTS_IN_FLIGHT / MAX_RUNS;
        for _ in 0..MAX_RUNS {
            let id = admit(&mut state, spec("http://127.0.0.1:1".into(), PER_RUN_COMPLETIONS + 1));
            ids.push(id);
            for ordinal in 0..PER_RUN_COMPLETIONS as u64 {
                let (worker, release) = held_cancelled_completion(&mut state, id, ordinal);
                workers.push(worker);
                releases.push(release);
            }
        }
        for _ in 0..state.events.max_capacity() {
            state.events.try_send(Event::Stopped).unwrap();
        }
        let operation_id = Uuid::new_v4();
        state
            .command(Command::SetIntruderPaused {
                project,
                run_id: ids[0],
                operation_id,
                paused: true,
            })
            .await
            .unwrap();
        let acknowledged = state.intruder_run(ids[0]).unwrap();
        state.store.reject_writes(true);
        let mut released = true;
        for release in releases {
            released &= release.send(()).is_ok();
        }
        let finished = workers_finished(&workers).await;
        let (returned_while_full, stayed_full) = {
            let mut waiting = Box::pin(state.event_permit());
            let mut granted = None;
            // All workers are ready; only the bounded lifecycle signals can still yield.
            for _ in 0..=MAX_REQUESTS_IN_FLIGHT + 2 * MAX_RUNS {
                if let Poll::Ready(permit) = poll_once(waiting.as_mut()).await {
                    granted = Some(permit);
                    break;
                }
                tokio::task::yield_now().await;
            }
            let returned_while_full = granted.is_some();
            let stayed_full = events.len() == events.max_capacity();
            let _ = events.try_recv();
            let permit = match granted {
                Some(permit) => permit,
                None => tokio::time::timeout(BOUND, waiting.as_mut()).await.unwrap(),
            };
            if let Some(permit) = permit {
                permit.send(Event::Stopped);
            }
            (returned_while_full, stayed_full)
        };
        let reclaimed_by_wait = !state.intruder.has_resources()
            && state.intruder.requests.is_empty();
        let queued = state.intruder.notifications.len();
        let _ = state.stop_intruder().await;
        let joined = state.finish_intruder_shutdown().await;
        while events.try_recv().is_ok() {}
        let mut delivered = Vec::new();
        while !state.intruder.can_accept_commands() {
            let signal = state.intruder.next_signal(&state.events, false).await;
            state.intruder_signal(signal).await.unwrap();
            while let Ok(event) = events.try_recv() {
                delivered.push(event);
            }
        }
        let before: Vec<_> = ids
            .iter()
            .map(|id| state.intruder_result_page(*id, None, PER_RUN_COMPLETIONS + 1).unwrap())
            .collect();
        state.store.reject_writes(false);
        let directory = tempfile::tempdir().unwrap();
        let saved = save(&mut state, Some(directory.path().join("bounded-recovery.mio"))).await;

        assert!(released && finished);
        joined.unwrap();
        saved.unwrap();
        assert!(!returned_while_full && stayed_full);
        assert!(reclaimed_by_wait, "capacity waiting must reclaim every ready sender and run transition");
        assert_eq!(MAX_NOTIFICATIONS, 2 + MAX_REQUESTS_IN_FLIGHT + 2 * MAX_RUNS);
        assert_eq!(queued, MAX_NOTIFICATIONS);
        assert_eq!(delivered.len(), MAX_NOTIFICATIONS);
        assert!(matches!(
            &delivered[0],
            Event::IntruderRunControl {
                project: owner,
                run_id,
                operation_id: operation,
                result: Ok(run),
            } if *owner == project && *run_id == ids[0]
                && *operation == operation_id && run == &acknowledged
        ));
        assert_eq!(delivered.iter().filter(|event| matches!(event, Event::IntruderRunControl { .. })).count(), 1);
        assert_eq!(delivered.iter().filter(|event| matches!(event, Event::HistoryChanged { project: owner } if *owner == project)).count(), 1);
        assert_eq!(delivered.iter().filter(|event| matches!(event, Event::Error { project: Some(owner), operation: "intruder", message } if *owner == project && !message.is_empty())).count(), MAX_REQUESTS_IN_FLIGHT + 2 * MAX_RUNS);
        for (id, before) in ids.into_iter().zip(before) {
            assert!(!before.persisted);
            assert!(matches!(before.run.state, IntruderRunState::Failed(_)));
            assert_eq!(
                (before.run.total, before.run.dispatched, before.run.completed),
                (
                    PER_RUN_COMPLETIONS as u64 + 1,
                    PER_RUN_COMPLETIONS as u64,
                    PER_RUN_COMPLETIONS as u64
                )
            );
            assert_eq!(before.rows.len(), PER_RUN_COMPLETIONS);
            assert!(before.rows.iter().all(|row| matches!(
                &row.outcome,
                IntruderResultOutcome::Flow(flow) if flow.state == FlowState::Cancelled
            )));
            let after = state
                .intruder_result_page(id, None, PER_RUN_COMPLETIONS + 1)
                .unwrap();
            assert!(after.persisted);
            assert_eq!((after.run, after.rows), (before.run, before.rows));
        }
        assert_eq!(state.unsaved_count(), 0);
        assert!(!state.intruder.has_resources());
        assert!(state.intruder.requests.is_empty());
    })
    .await
    .expect("the maximum admitted recovery fixture has no sockets or timed worker sleeps");
}
