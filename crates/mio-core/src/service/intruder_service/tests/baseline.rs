use super::*;

async fn finish_next(state: &mut State) {
    let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
    state.complete_intruder(completion).await.unwrap();
}

#[tokio::test]
async fn completed_baseline_preparation_reads_one_frozen_spec_without_messages() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            for expected in [255, b'X'] {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert_eq!(request(&mut stream).await, expected);
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK").await.unwrap();
            }
            listener
        });
        let (mut state, _events) = state();
        let input = spec(format!("http://{address}"), 1);
        let id = admit(&mut state, input.clone());
        state.advance_intruder(id).await.unwrap();
        finish_next(&mut state).await;
        state.advance_intruder(id).await.unwrap();
        let before = state.intruder_run(id).unwrap();
        assert_eq!(before.state, IntruderRunState::Completed);

        // The authorizer counts compiled column accesses, not physical disk reads.
        // Calibrate against one full read rather than hardcoding SPEC/PAYLOAD_QUERY expressions.
        let reads = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
        let observed = Arc::clone(&reads);
        state.store.test_intruder_connection(id).unwrap().authorizer(Some(move |context: AuthContext<'_>| {
            match context.action {
                AuthAction::Read { table_name: "intruder_runs", column_name: "template", .. } => {
                    observed[0].fetch_add(1, Ordering::Relaxed);
                }
                AuthAction::Read { table_name: "intruder_payloads", column_name: "value", .. } => {
                    observed[1].fetch_add(1, Ordering::Relaxed);
                }
                AuthAction::Read { table_name: "messages", column_name: "head" | "body", .. } => return Authorization::Deny,
                _ => {}
            }
            Authorization::Allow
        }));
        assert_eq!(state.store.read_intruder_spec(id).unwrap(), input);
        let once = reads.each_ref().map(|count| count.swap(0, Ordering::Relaxed));
        assert!(once.iter().all(|count| *count > 0));
        let receipt = state.request_intruder_baseline(state.project, id, Uuid::new_v4(), before.revision, &CancellationToken::new()).unwrap();
        let actual = reads.each_ref().map(|count| count.load(Ordering::Relaxed));
        state.store.test_intruder_connection(id).unwrap().authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        assert_eq!(receipt.run.baseline, Some(IntruderBaseline { ordinal: 1, completed: false }));
        state.advance_intruder(id).await.unwrap();
        finish_next(&mut state).await;
        state.advance_intruder(id).await.unwrap();
        assert_eq!(state.intruder_run(id).unwrap().state, IntruderRunState::Completed);
        assert!(!state.intruder.has_resources());
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
        assert_eq!(actual, once, "Completed baseline must load the owned frozen template/payloads once for validation and sender preparation");
    }).await.expect("single frozen-spec read budget and one baseline socket are bounded");
}

#[tokio::test]
async fn initial_baseline_shares_concurrency_and_payload_progress_excludes_its_pending_result() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (all_seen_tx, all_seen) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let mut baseline = None;
            let mut bodies = Vec::new();
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let byte = request(&mut stream).await;
                bodies.push(byte);
                if byte == b'X' {
                    assert!(baseline.replace(stream).is_none());
                } else {
                    assert_eq!(byte, 255);
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                        )
                        .await
                        .unwrap();
                }
            }
            bodies.sort_unstable();
            assert_eq!(bodies, [b'X', 255, 255]);
            all_seen_tx.send(()).unwrap();
            released.await.unwrap();
            baseline
                .unwrap()
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
                .await
                .unwrap();
            listener
        });
        let (mut state, _events) = state();
        let mut input = spec(format!("http://{address}"), 2);
        input.save_to_project = false;
        input.baseline_request = true;
        input.max_concurrent_requests = 3;
        let id = admit(&mut state, input);
        for _ in 0..3 {
            state.advance_intruder(id).await.unwrap();
        }
        all_seen.await.unwrap();
        finish_next(&mut state).await;
        finish_next(&mut state).await;
        let pending = state.intruder_result_page(id, None, 10).unwrap();
        assert_eq!(pending.run.state, IntruderRunState::Running);
        assert_eq!(
            (
                pending.run.dispatched,
                pending.run.completed,
                pending.run.total
            ),
            (3, 2, 3)
        );
        assert_eq!(pending.run.payload_progress(), Some((2, 2)));
        assert_eq!(
            pending.run.baseline,
            Some(IntruderBaseline {
                ordinal: 0,
                completed: false
            })
        );
        assert_eq!(state.intruder.jobs.len(), 1);
        assert!(state.intruder.advance_candidate().is_none());
        release.send(()).unwrap();
        finish_next(&mut state).await;
        state.advance_intruder(id).await.unwrap();
        let completed = state.intruder_result_page(id, None, 10).unwrap();
        assert_eq!(completed.run.state, IntruderRunState::Completed);
        assert_eq!(completed.run.payload_progress(), Some((2, 2)));
        assert_eq!(
            completed.run.baseline,
            Some(IntruderBaseline {
                ordinal: 0,
                completed: true
            })
        );
        assert!(!completed.persisted);
        assert!(!state.intruder.has_resources());
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    })
    .await
    .expect("payloads can complete while the single baseline response remains held");
}

#[tokio::test]
async fn requested_baseline_preserves_paused_token_and_inserts_before_only_unsent_payloads() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, mut seen) = mpsc::channel(3);
        let origin = tokio::spawn(async move {
            for expected in [255, b'X', 255] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let byte = request(&mut stream).await;
                assert_eq!(byte, expected);
                seen_tx.send(byte).await.unwrap();
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                    )
                    .await
                    .unwrap();
            }
            listener
        });
        let (mut state, _events) = state();
        let mut input = spec(format!("http://{address}"), 2);
        input.save_to_project = false;
        input.max_concurrent_requests = 1;
        let id = admit(&mut state, input.clone());
        state.advance_intruder(id).await.unwrap();
        assert_eq!(seen.recv().await, Some(255));
        finish_next(&mut state).await;
        let paused = state.set_intruder_paused(state.project, id, true).unwrap();
        let admission = CancellationToken::new();
        let receipt = state
            .request_intruder_baseline(
                state.project,
                id,
                Uuid::new_v4(),
                paused.revision,
                &admission,
            )
            .unwrap();
        assert_eq!(receipt.run.state, IntruderRunState::Paused);
        assert_eq!(
            (
                receipt.run.dispatched,
                receipt.run.completed,
                receipt.run.total
            ),
            (1, 1, 3)
        );
        assert_eq!(
            receipt.run.baseline,
            Some(IntruderBaseline {
                ordinal: 1,
                completed: false
            })
        );
        assert_eq!(state.store.read_intruder_spec(id).unwrap(), input);
        admission.cancel();
        assert!(!receipt.cancel.is_cancelled());
        assert!(!state.intruder.runs[&id].cancel.is_cancelled());
        assert!(state.intruder.advance_candidate().is_none());
        assert!(seen.try_recv().is_err());
        assert!(
            state
                .request_intruder_baseline(
                    state.project,
                    id,
                    Uuid::new_v4(),
                    receipt.run.revision,
                    &CancellationToken::new()
                )
                .is_err()
        );
        state.set_intruder_paused(state.project, id, false).unwrap();
        state.advance_intruder(id).await.unwrap();
        assert_eq!(seen.recv().await, Some(b'X'));
        finish_next(&mut state).await;
        let baseline_done = state.intruder_result_page(id, None, 10).unwrap();
        assert_eq!(baseline_done.run.payload_progress(), Some((1, 2)));
        assert_eq!(
            baseline_done.run.baseline,
            Some(IntruderBaseline {
                ordinal: 1,
                completed: true
            })
        );
        state.advance_intruder(id).await.unwrap();
        assert_eq!(seen.recv().await, Some(255));
        finish_next(&mut state).await;
        state.advance_intruder(id).await.unwrap();
        let final_page = state.intruder_result_page(id, None, 10).unwrap();
        assert_eq!(final_page.run.state, IntruderRunState::Completed);
        assert_eq!(final_page.run.payload_progress(), Some((2, 2)));
        assert_eq!(
            final_page
                .rows
                .iter()
                .map(|row| row.ordinal)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    })
    .await
    .expect("one late baseline must not resume a paused attack or replay a completed payload");
}

#[tokio::test]
async fn completed_baseline_command_uses_frozen_plan_and_new_run_token_without_payload_replay() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            for expected in [255, b'X'] {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert_eq!(request(&mut stream).await, expected);
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                    )
                    .await
                    .unwrap();
            }
            listener
        });
        let (mut state, mut events) = state();
        let input = spec(format!("http://{address}"), 1);
        let id = admit(&mut state, input.clone());
        let old_token = state.intruder.runs[&id].cancel.clone();
        state.advance_intruder(id).await.unwrap();
        finish_next(&mut state).await;
        state.advance_intruder(id).await.unwrap();
        let completed = state.intruder_result_page(id, None, 10).unwrap();
        assert_eq!(completed.run.state, IntruderRunState::Completed);
        let old_row = completed.rows[0].clone();
        old_token.cancel();
        let mut workspace = state.store.workspace().unwrap();
        workspace.revision += 1;
        workspace
            .intruder
            .closed
            .push(workspace.intruder.tabs.remove(0));
        workspace.intruder.active = None;
        state.store.save_workspace(&workspace).unwrap();
        while events.try_recv().is_ok() {}
        let operation_id = Uuid::new_v4();
        let admission = CancellationToken::new();
        state
            .command(Command::RequestIntruderBaseline {
                project: state.project,
                run_id: id,
                operation_id,
                expected_revision: completed.run.revision,
                cancel: admission.clone(),
            })
            .await
            .unwrap();
        let Event::IntruderBaselineRequested {
            project,
            run_id,
            operation_id: actual,
            result: Ok(receipt),
        } = events.try_recv().unwrap()
        else {
            panic!("RequestBaseline must return one correlated accepted receipt");
        };
        assert_eq!((project, run_id, actual), (state.project, id, operation_id));
        assert_eq!(receipt.run.state, IntruderRunState::Running);
        assert_eq!(
            (
                receipt.run.dispatched,
                receipt.run.completed,
                receipt.run.total
            ),
            (1, 1, 2)
        );
        assert_eq!(
            receipt.run.baseline,
            Some(IntruderBaseline {
                ordinal: 1,
                completed: false
            })
        );
        admission.cancel();
        assert!(!receipt.cancel.is_cancelled());
        assert!(!state.intruder.runs[&id].cancel.is_cancelled());
        assert_eq!(state.store.read_intruder_spec(id).unwrap(), input);
        assert_eq!(state.store.workspace().unwrap(), workspace);
        state.advance_intruder(id).await.unwrap();
        finish_next(&mut state).await;
        state.advance_intruder(id).await.unwrap();
        let final_page = state.intruder_result_page(id, None, 10).unwrap();
        assert_eq!(final_page.run.state, IntruderRunState::Completed);
        assert_eq!(final_page.rows[0], old_row);
        assert_eq!(final_page.run.payload_progress(), Some((1, 1)));
        assert_eq!(
            final_page.run.baseline,
            Some(IntruderBaseline {
                ordinal: 1,
                completed: true
            })
        );
        assert!(
            state
                .request_intruder_baseline(
                    state.project,
                    id,
                    Uuid::new_v4(),
                    final_page.run.revision,
                    &CancellationToken::new()
                )
                .is_err()
        );
        assert!(!state.intruder.has_resources());
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    })
    .await
    .expect("completed baseline reactivation sends only the original template under a fresh token");
}

#[test]
fn baseline_only_empty_payload_start_and_cancelled_or_stale_requests_have_no_side_effects() {
    let (mut state, _events) = state();
    let mut empty = spec("http://127.0.0.1:1".into(), 0);
    empty.baseline_request = true;
    let tab = bind(&mut state.store, &empty);
    let rejected = Uuid::new_v4();
    assert!(
        state
            .admit_intruder(
                state.project,
                rejected,
                tab,
                empty,
                CancellationToken::new()
            )
            .is_err()
    );
    assert!(state.store.find_intruder_run(rejected).unwrap().is_none());
    let id = admit(&mut state, spec("http://127.0.0.1:1".into(), 1));
    let before = state.intruder.runs[&id].summary.clone();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    for (operation, revision, token) in [
        (Uuid::new_v4(), before.revision, cancelled),
        (Uuid::nil(), before.revision, CancellationToken::new()),
        (
            Uuid::new_v4(),
            before.revision + 1,
            CancellationToken::new(),
        ),
    ] {
        assert!(
            state
                .request_intruder_baseline(state.project, id, operation, revision, &token)
                .is_err()
        );
        assert_eq!(state.store.read_intruder_run(id).unwrap(), before);
        assert_eq!(state.intruder.runs[&id].summary, before);
        assert!(!state.intruder.runs[&id].cancel.is_cancelled());
        assert!(state.intruder.jobs.is_empty() && state.intruder.requests.is_empty());
        assert!(state.intruder.recovery.is_empty());
    }
    state.request_intruder_stop(state.project, id).unwrap();
    state
        .persist_intruder_state(id, IntruderRunState::Stopping)
        .unwrap();
    let stopping = state.intruder.runs[&id].summary.clone();
    assert!(
        state
            .request_intruder_baseline(
                state.project,
                id,
                Uuid::new_v4(),
                stopping.revision,
                &CancellationToken::new()
            )
            .is_err()
    );
    assert_eq!(state.store.read_intruder_run(id).unwrap(), stopping);
}

#[tokio::test]
async fn cancelled_baseline_write_recovery_preserves_terminal_role_without_replaying_payloads() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, seen) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut stream).await, b'X');
            seen_tx.send(()).unwrap();
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
            listener
        });
        let (mut state, _events) = state();
        let mut input = spec(format!("http://{address}"), 1);
        input.baseline_request = true;
        input.max_concurrent_requests = 1;
        let id = admit(&mut state, input);
        state.advance_intruder(id).await.unwrap();
        seen.await.unwrap();
        state.store.reject_writes(true);
        state.intruder.runs[&id].cancel.cancel();
        let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
        assert!(state.complete_intruder(completion).await.is_err());
        assert!(state.stop_intruder().await.is_err());
        assert!(!state.intruder.has_resources());
        let before = state.intruder_result_page(id, None, 10).unwrap();
        assert_eq!(before.run.baseline, Some(IntruderBaseline { ordinal: 0, completed: true }));
        assert_eq!(before.run.payload_progress(), Some((0, 1)));
        assert_eq!((before.run.dispatched, before.run.completed, before.rows.len()), (1, 1, 1));
        assert!(!before.persisted);
        assert!(matches!(&before.rows[0].outcome, IntruderResultOutcome::Flow(flow) if flow.state == FlowState::Cancelled));
        assert!(!state.intruder.recovery.is_empty());
        state.store.reject_writes(false);
        let directory = tempfile::tempdir().unwrap();
        save(&mut state, Some(directory.path().join("baseline-recovery.mio"))).await.unwrap();
        let after = state.intruder_result_page(id, None, 10).unwrap();
        assert_eq!(after.run, before.run);
        assert_eq!(after.rows, before.rows);
        assert!(after.persisted);
        assert!(state.intruder.recovery.is_empty());
        assert!(state.request_intruder_baseline(state.project, id, Uuid::new_v4(), after.run.revision, &CancellationToken::new()).is_err());
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    }).await.expect("cancelled baseline must close its socket and recover only its known terminal result");
}
