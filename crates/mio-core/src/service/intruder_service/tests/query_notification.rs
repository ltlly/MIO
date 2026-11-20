use std::{future::poll_fn, task::Poll};

use super::*;

#[tokio::test]
async fn querying_another_run_with_a_full_event_queue_preserves_unnotified_completion() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let (mut state, mut events) = state();
        let project = state.project;
        let finished = admit(&mut state, spec("http://127.0.0.1:1".into(), 1));
        let paused = admit(&mut state, spec("http://127.0.0.1:1".into(), 1));
        state.set_intruder_paused(project, paused, true).unwrap();
        let mut query = IntruderResultViewQuery {
            project,
            run_id: paused,
            view_id: Uuid::new_v4(),
            query_revision: 1,
            offset: 0,
            limit: 1,
            filter: IntruderResultFilter::default(),
            sort: IntruderResultSort::default(),
            selected_ordinal: None,
            grep_config: crate::IntruderGrepConfig::default(),
        };

        let mut completed = crate::service::tests::completed_flow();
        completed.summary.source = TrafficSource::Intruder;
        let flow_id = completed.summary.id;
        let mut pending = completed.clone();
        pending.summary.revision = 1;
        pending.summary.state = FlowState::Pending;
        pending.summary.status = None;
        pending.summary.response_bytes = 0;
        pending.response = None;
        state
            .persist_intruder_result(finished, 0, Ok(Box::new(pending)))
            .unwrap();
        let (release, released) = oneshot::channel();
        let worker = state.intruder.jobs.spawn(async move {
            released.await.unwrap();
            Completion {
                project,
                record: completed,
                reason_phrase: None,
            }
        });
        state.intruder.requests.insert(
            worker.id(),
            ActiveRequest { run_id: finished, ordinal: 0, flow_id, resource_pool: None },
        );
        state.intruder.runs.get_mut(&finished).unwrap().active += 1;

        let initial = state.intruder.next_signal(&state.events, false).await;
        assert!(matches!(initial, Signal::Notify(_)));
        state.intruder_signal(initial).await.unwrap();
        assert!(matches!(events.try_recv(), Ok(Event::IntruderChanged { project: owner }) if owner == project));
        state.command(Command::QueryIntruderResultView {
            query: query.clone(),
            cancel: CancellationToken::new(),
        }).await.unwrap();
        assert!(matches!(events.try_recv(), Ok(Event::IntruderResultView { query: reply, result: Ok(_) }) if reply == query));
        assert!(events.try_recv().is_err());
        assert!(!state.intruder.dirty && !state.intruder.notified);

        release.send(()).unwrap();
        let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
        state.complete_intruder(completion).await.unwrap();
        state.advance_intruder(finished).await.unwrap();
        assert!(!state.intruder.runs.contains_key(&finished));
        assert!(state.intruder.jobs.is_empty() && state.intruder.requests.is_empty());
        let finished_summary = state.store.read_intruder_run(finished).unwrap();
        assert_eq!(finished_summary.state, IntruderRunState::Completed);
        assert_eq!((finished_summary.dispatched, finished_summary.completed), (1, 1));
        assert!(state.intruder.dirty && !state.intruder.notified);
        while events.try_recv().is_ok() {}
        assert!(state.intruder.notifications.is_empty());
        for _ in 0..state.events.max_capacity() {
            state.events.try_send(Event::Stopped).unwrap();
        }
        assert!(state.intruder.can_accept_commands());
        {
            let mut notice = std::pin::pin!(state.intruder.next_signal(&state.events, false));
            assert!(poll_fn(|cx| Poll::Ready(notice.as_mut().poll(cx))).await.is_pending());
        }

        query.query_revision += 1;
        {
            let mut command = std::pin::pin!(state.command(Command::QueryIntruderResultView {
                query: query.clone(),
                cancel: CancellationToken::new(),
            }));
            assert!(poll_fn(|cx| Poll::Ready(command.as_mut().poll(cx))).await.is_pending());
            assert!(matches!(events.try_recv(), Ok(Event::Stopped)));
            command.await.unwrap();
        }
        for _ in 1..state.events.max_capacity() {
            assert!(matches!(events.try_recv(), Ok(Event::Stopped)));
        }
        let Event::IntruderResultView { query: reply, result } = events.try_recv().unwrap() else {
            panic!("the released slot belongs to the original result query");
        };
        assert_eq!(reply, query);
        let page = result.unwrap();
        assert_eq!((page.run.id, page.run.state), (paused, IntruderRunState::Paused));
        assert!(page.rows.is_empty() && page.total == 0);
        assert!(events.try_recv().is_err());
        assert!(!state.intruder.runs.contains_key(&finished));

        let signal = {
            let mut notice = std::pin::pin!(state.intruder.next_signal(&state.events, false));
            poll_fn(|cx| Poll::Ready(notice.as_mut().poll(cx))).await
        };
        let Poll::Ready(signal @ Signal::Notify(_)) = signal else {
            panic!("querying run B must not acknowledge run A's unnotified terminal result");
        };
        state.intruder_signal(signal).await.unwrap();
        assert!(matches!(events.try_recv(), Ok(Event::IntruderChanged { project: owner }) if owner == project));
        assert!(!state.intruder.dirty && state.intruder.notified);

        query.query_revision += 1;
        state.command(Command::QueryIntruderResultView {
            query: query.clone(),
            cancel: CancellationToken::new(),
        }).await.unwrap();
        assert!(matches!(events.try_recv(), Ok(Event::IntruderResultView { query: reply, result: Ok(_) }) if reply == query));
        assert!(!state.intruder.dirty && !state.intruder.notified);
        {
            let mut notice = std::pin::pin!(state.intruder.next_signal(&state.events, false));
            assert!(poll_fn(|cx| Poll::Ready(notice.as_mut().poll(cx))).await.is_pending(), "acknowledging a delivered notice must not create an endless refresh loop");
        }
        assert_eq!(state.store.read_intruder_run(finished).unwrap(), finished_summary);
        assert_eq!(state.intruder.runs[&paused].summary.dispatched, 0);
        state.stop_intruder().await.unwrap();
        assert!(!state.intruder.has_resources());
    }).await.expect("two in-memory runs and notification delivery are bounded without network or sleeps");
}
