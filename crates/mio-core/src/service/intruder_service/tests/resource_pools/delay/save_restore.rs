use super::*;

#[tokio::test]
async fn cancellation_during_pending_commit_does_not_consume_a_pool_send_or_deadline() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    tokio::time::timeout(Duration::from_secs(3), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (mut state, _events) = state();
        let mut shared = pool("cancelled before dispatch", Some(1));
        shared.delay = Some(IntruderPoolDelay::Increment {
            initial_ms: 100,
            increment_ms: 100,
        });
        install(&mut state, vec![shared.clone()]);
        let mut input = spec(format!("http://{}", listener.local_addr().unwrap()), 1);
        input.resource_pool = Some(IntruderPoolSelection::Existing(shared.id));
        let id = admit(&mut state, input);
        let before = state.intruder.pool_timings[&shared.id];
        let cancel = state.intruder.runs[&id].cancel.clone();
        let cancelled_during_pending = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&cancelled_during_pending);
        let token = cancel.clone();
        state
            .store
            .test_intruder_connection(id)
            .unwrap()
            .authorizer(Some(move |context: AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Update {
                        table_name: "intruder_runs",
                        column_name: "dispatched",
                        ..
                    }
                ) {
                    observed.store(true, Ordering::SeqCst);
                    token.cancel();
                }
                Authorization::Allow
            }));
        state.advance_intruder(id).await.unwrap();
        state
            .store
            .test_intruder_connection(id)
            .unwrap()
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        assert!(
            cancelled_during_pending.load(Ordering::SeqCst),
            "the token was cancelled inside the Pending write, before dispatch resumes"
        );
        assert!(cancel.is_cancelled());
        while !state.intruder.jobs.is_empty() {
            finish_next(&mut state).await;
        }
        state.stop_intruder().await.unwrap();
        let page = state.intruder_result_page(id, None, 2).unwrap();
        assert_eq!(page.run.state, IntruderRunState::Stopped);
        assert_eq!(
            (page.run.dispatched, page.run.completed, page.rows.len()),
            (1, 1, 1)
        );
        let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else {
            panic!("the already committed Pending row must become Cancelled")
        };
        assert_eq!(summary.state, FlowState::Cancelled);
        assert!(state.store.read(summary.id).unwrap().response.is_none());
        assert!(!state.intruder.has_resources());
        assert!(state.intruder.requests.is_empty());
        no_extra(listener);
        assert_eq!(
            state.intruder.pool_timings[&shared.id], before,
            "observable cancellation before the network attempt must not charge the shared pool"
        );
    })
    .await
    .expect("cancellation during Pending commit creates no socket and leaves no sender owned");
}
