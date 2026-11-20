use super::*;

#[tokio::test]
async fn failed_pause_or_resume_never_ack_success_and_saving_cannot_replay_the_control() {
    tokio::time::timeout(Duration::from_secs(5), async {
        for paused in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let (mut state, mut events) = state();
            let project = state.project;
            let directory = tempfile::tempdir().unwrap();
            save(
                &mut state,
                Some(directory.path().join("control-write-fault.mio")),
            )
            .await
            .unwrap();
            let id = admit(
                &mut state,
                spec(format!("http://{}", listener.local_addr().unwrap()), 2),
            );
            if !paused {
                control(&mut state, &mut events, project, id, true)
                    .await
                    .unwrap();
            }
            let before = state.intruder_run(id).unwrap();
            state.store.reject_writes(true);
            let error = control(&mut state, &mut events, project, id, paused)
                .await
                .unwrap_err();
            assert!(error.contains("未确认"));
            assert_eq!(
                state.intruder_run(id).unwrap(),
                before,
                "uncommitted Resume must not make the scheduler Running"
            );
            assert_eq!(state.store.read_intruder_run(id).unwrap(), before);
            assert!(state.intruder.runs[&id].cancel.is_cancelled());
            let first_storage_error = state.storage_error.clone().unwrap();
            assert!(
                control(&mut state, &mut events, project, id, false)
                    .await
                    .is_err()
            );
            assert!(state.stop_intruder().await.is_err());
            assert!(!state.intruder.has_resources());
            let stopped_storage_error = state.storage_error.clone();
            let failed = state.intruder_result_page(id, None, 1).unwrap();
            assert!(!failed.persisted);
            assert!(matches!(failed.run.state, IntruderRunState::Failed(_)));
            assert_eq!((failed.run.dispatched, failed.run.completed), (0, 0));
            assert!(save(&mut state, None).await.is_err());
            state.store.reject_writes(false);
            save(&mut state, None).await.unwrap();
            assert!(state.intruder_result_page(id, None, 1).unwrap().persisted);
            assert_eq!(state.store.read_intruder_run(id).unwrap(), failed.run);
            assert_eq!(state.unsaved_count(), 0);
            assert_eq!(
                stopped_storage_error.as_ref(),
                Some(&first_storage_error),
                "recovery bookkeeping must retain the original storage failure"
            );
            assert!(
                control(&mut state, &mut events, project, id, false)
                    .await
                    .is_err()
            );
            assert_eq!(
                listener.into_std().unwrap().accept().unwrap_err().kind(),
                ErrorKind::WouldBlock
            );
        }
    })
    .await
    .expect("failed controls only persist bounded recovery and never open a socket");
}

#[tokio::test]
async fn control_errors_keep_project_run_operation_identity_and_do_not_affect_other_runs() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let (mut state, mut events) = state();
        let project = state.project;
        let id = admit(&mut state, spec("http://127.0.0.1:1".into(), 1));
        let other = admit(&mut state, spec("http://127.0.0.1:1".into(), 1));
        let before = state.intruder_run(id).unwrap();
        let other_before = state.intruder_run(other).unwrap();
        let stale = ProjectKey {
            generation: project.generation + 1,
            ..project
        };
        assert!(
            control(&mut state, &mut events, stale, id, true)
                .await
                .is_err()
        );
        assert!(
            control(&mut state, &mut events, project, Uuid::new_v4(), true)
                .await
                .is_err()
        );
        state.quiesced = true;
        assert!(
            control(&mut state, &mut events, project, id, true)
                .await
                .is_err()
        );
        state.quiesced = false;
        assert_eq!(state.intruder_run(id).unwrap(), before);
        assert_eq!(state.intruder_run(other).unwrap(), other_before);
        let paused = control(&mut state, &mut events, project, id, true)
            .await
            .unwrap();
        assert_eq!(
            control(&mut state, &mut events, project, id, true)
                .await
                .unwrap(),
            paused,
            "duplicate Pause is revision-idempotent"
        );
        state.request_intruder_stop(project, id).unwrap();
        assert!(
            control(&mut state, &mut events, project, id, false)
                .await
                .is_err()
        );
        assert_eq!(state.intruder_run(other).unwrap(), other_before);
        assert!(!state.intruder.runs[&other].cancel.is_cancelled());
        state.stop_intruder().await.unwrap();
        assert!(!state.intruder.has_resources());
    })
    .await
    .expect("invalid control identities are rejected without network work");
}
