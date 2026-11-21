use super::*;

#[tokio::test]
async fn pending_cancelled_session_write_failure_recovers_its_reserved_result_without_sending() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (mut state, _events) = state();
        let mut shared = pool("Session cancellation recovery", Some(1));
        shared.delay = Some(IntruderPoolDelay::Fixed { milliseconds: 100 });
        install(&mut state, vec![shared.clone()]);
        let mut input = spec(format!("http://{}", listener.local_addr().unwrap()), 1);
        input.save_to_project = false;
        input.resource_pool = Some(IntruderPoolSelection::Existing(shared.id));
        let expected_request = parse_request(b"POST /intruder HTTP/1.1\r\nContent-Length: 1\r\n\r\n\xff", &input.draft.target, input.draft.version).unwrap();
        let id = admit(&mut state, input);
        let timing = state.intruder.pool_timings[&shared.id];
        let cancel = state.intruder.runs[&id].cancel.clone();
        let pending_committed = Arc::new(AtomicBool::new(false));
        let terminal_rejected = Arc::new(AtomicBool::new(false));
        let observed_pending = Arc::clone(&pending_committed);
        let observed_rejected = Arc::clone(&terminal_rejected);
        state.store.set_session_intruder_write_hook(id, Some(Arc::new(move |action: &str| {
            if action == "dispatched" {
                observed_pending.store(true, Ordering::SeqCst);
                cancel.cancel();
                return true;
            }
            if observed_pending.load(Ordering::SeqCst) && action == "insert_flow" {
                observed_rejected.store(true, Ordering::SeqCst);
                return false;
            }
            true
        })));
        assert!(state.advance_intruder(id).await.is_err());
        state.store.set_session_intruder_write_hook(id, None);
        assert!(pending_committed.load(Ordering::SeqCst) && terminal_rejected.load(Ordering::SeqCst));
        assert!(state.stop_intruder().await.is_err());
        assert!(!state.intruder.has_resources());
        assert!(state.intruder.requests.is_empty());
        let before = state.intruder_result_page(id, None, 2).unwrap();
        assert_eq!((before.run.dispatched, before.run.completed, before.rows.len()), (1, 1, 1));
        let IntruderResultOutcome::Flow(summary) = &before.rows[0].outcome else { panic!("Cancelled recovery must preserve the committed flow"); };
        assert_eq!(summary.state, FlowState::Cancelled);
        let record = state.read_flow(summary.id).unwrap();
        assert_eq!(record.request, expected_request);
        assert!(record.response.is_none());
        assert!(!state.intruder.recovery.is_empty());
        let recovery_len = state.intruder.recovery.len();
        state.store.reject_session_intruder_writes(id, true);
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let path = directory.path().join("cancel-recovery.mio");
        assert!(save(&mut state, Some(path.clone())).await.is_err());
        assert_eq!(state.intruder.recovery.len(), recovery_len);
        assert!(!path.exists());
        state.store.reject_session_intruder_writes(id, false);
        // Session replay requires the original flow reservation; losing it makes this Save fail.
        save(&mut state, Some(path.clone())).await.unwrap();
        assert!(state.intruder.recovery.is_empty());
        assert_eq!(state.intruder_result_page(id, None, 2).unwrap(), before);
        assert_eq!(state.store.read(summary.id).unwrap(), record);
        // Terminal state writes require every result reservation to have been released.
        assert_eq!(state.store.set_intruder_state(id, before.run.state.clone()).unwrap(), before.run);
        let disk = ProjectStore::open(&path).unwrap();
        assert!(disk.find_intruder_run(id).unwrap().is_none());
        assert!(disk.read_optional(summary.id).unwrap().is_none());
        disk.close().unwrap();
        assert_eq!(state.intruder.pool_timings[&shared.id], timing);
        no_extra(listener);
    }).await.expect("Pending cancellation and failed RAM completion recover only the reserved result, never a socket");
}
