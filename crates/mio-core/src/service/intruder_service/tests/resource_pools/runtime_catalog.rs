use super::*;

#[tokio::test]
async fn resource_pool_command_receipts_reject_cancelled_or_stale_edits_and_reload_on_project_switch()
 {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut state, mut events) = state();
        let project = state.project;
        let shared = pool("first project", None);
        let other = pool("other pool", Some(1));
        let operation = Uuid::new_v4();
        state
            .command(Command::SetIntruderResourcePools {
                project,
                operation_id: operation,
                expected_revision: 0,
                pools: vec![shared.clone(), other.clone()],
                cancel: CancellationToken::new(),
            })
            .await
            .unwrap();
        let Event::IntruderResourcePoolsUpdated {
            project: owner,
            operation_id,
            result: Ok(catalog),
        } = events.try_recv().unwrap()
        else {
            panic!("catalog OK must have one correlated receipt")
        };
        assert_eq!(
            (owner, operation_id, catalog.revision),
            (project, operation, 1)
        );
        for (expected_revision, cancel) in [
            (0, CancellationToken::new()),
            (1, {
                let cancel = CancellationToken::new();
                cancel.cancel();
                cancel
            }),
        ] {
            let operation_id = Uuid::new_v4();
            state
                .command(Command::SetIntruderResourcePools {
                    project,
                    operation_id,
                    expected_revision,
                    pools: vec![],
                    cancel,
                })
                .await
                .unwrap();
            let Event::IntruderResourcePoolsUpdated {
                operation_id: actual,
                result,
                ..
            } = events.try_recv().unwrap()
            else {
                panic!("rejected catalog edit remains correlated")
            };
            assert_eq!(actual, operation_id);
            assert!(result.is_err());
            assert_eq!(state.intruder.resource_pools, catalog);
            assert_eq!(state.store.intruder_resource_pools().unwrap(), catalog);
        }
        let mut input = spec("http://127.0.0.1:1".into(), 1);
        input.resource_pool = Some(IntruderPoolSelection::Existing(shared.id));
        let run = admit(&mut state, input);
        let original = state.intruder_run(run).unwrap();
        let operation_id = Uuid::new_v4();
        state
            .command(Command::SetIntruderResourcePool {
                project,
                run_id: run,
                operation_id,
                expected_pool_revision: original.resource_pool_revision,
                resource_pool: shared.id,
                cancel: CancellationToken::new(),
            })
            .await
            .unwrap();
        let Event::IntruderResourcePoolChanged {
            project: owner,
            run_id,
            operation_id: actual,
            result,
        } = events.try_recv().unwrap()
        else {
            panic!("binding refusal remains correlated")
        };
        assert_eq!((owner, run_id, actual), (project, run, operation_id));
        assert!(result.is_err(), "same-pool mutation is explicitly rejected");
        assert_eq!(state.intruder_run(run).unwrap(), original);
        for (expected_pool_revision, cancel) in [
            (0, CancellationToken::new()),
            (1, {
                let cancel = CancellationToken::new();
                cancel.cancel();
                cancel
            }),
        ] {
            let operation_id = Uuid::new_v4();
            state
                .command(Command::SetIntruderResourcePool {
                    project,
                    run_id: run,
                    operation_id,
                    expected_pool_revision,
                    resource_pool: other.id,
                    cancel,
                })
                .await
                .unwrap();
            let Event::IntruderResourcePoolChanged {
                operation_id: actual,
                result,
                ..
            } = events.try_recv().unwrap()
            else {
                panic!("stale/cancelled binding must receive a refusal")
            };
            assert_eq!(actual, operation_id);
            assert!(result.is_err());
            assert_eq!(state.intruder_run(run).unwrap(), original);
        }
        let operation_id = Uuid::new_v4();
        state
            .command(Command::SetIntruderResourcePool {
                project,
                run_id: run,
                operation_id,
                expected_pool_revision: 1,
                resource_pool: other.id,
                cancel: CancellationToken::new(),
            })
            .await
            .unwrap();
        let Event::IntruderResourcePoolChanged {
            operation_id: actual,
            result: Ok(rebound),
            ..
        } = events.try_recv().unwrap()
        else {
            panic!("successful binding must receive its updated summary")
        };
        assert_eq!(actual, operation_id);
        assert_eq!(
            (
                rebound.resource_pool,
                rebound.resource_pool_revision,
                rebound.revision
            ),
            (Some(other.id), 2, original.revision)
        );
        assert_eq!(state.intruder_run(run).unwrap(), rebound);
        state.stop_intruder().await.unwrap();
        while events.try_recv().is_ok() {}
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let mut next = ProjectStore::create(Some(&directory.path().join("next.mio"))).unwrap();
        let replacement = next
            .set_intruder_resource_pools(
                ProjectKey {
                    id: next.id,
                    generation: project.generation + 1,
                },
                0,
                vec![pool("next project", Some(1))],
                &CancellationToken::new(),
            )
            .unwrap();
        state.replace_project(next).await.unwrap();
        while events.try_recv().is_ok() {}
        for (owner, revision) in [(state.project, 80), (project, 81)] {
            state
                .command(Command::QueryIntruderResourcePools {
                    project: owner,
                    query_revision: revision,
                })
                .await
                .unwrap();
            let Event::IntruderResourcePools {
                project: actual,
                query_revision,
                result,
            } = events.try_recv().unwrap()
            else {
                panic!("query receipt identifies its project and revision")
            };
            assert_eq!((actual, query_revision), (owner, revision));
            if owner == state.project {
                assert_eq!(result.unwrap(), replacement);
            } else {
                assert!(result.is_err());
            }
        }
        assert_eq!(state.intruder.resource_pools, replacement);
        assert!(!state.intruder.has_resources());
    })
    .await
    .expect(
        "catalog and binding receipts, rejected edits and project cache replacement are bounded",
    );
}
