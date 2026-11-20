use super::*;

#[tokio::test]
async fn partial_session_pool_delete_keeps_actual_bindings_through_results_stop_and_save_as() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, seen) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let mut streams = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert_eq!(request(&mut stream).await, 255);
                streams.push(stream);
            }
            seen_tx.send(()).unwrap();
            released.await.unwrap();
            for mut stream in streams {
                respond(&mut stream).await;
            }
            listener
        });
        let (mut state, _events) = state();
        let catalog = state
            .ensure_intruder_default_pool(
                state.project,
                Uuid::new_v4(),
                &CancellationToken::new(),
            )
            .unwrap();
        let default = catalog.default_pool_id.unwrap();
        let old = pool("partial delete", Some(2));
        let mut pools = catalog.pools;
        pools.push(old.clone());
        let catalog = install(&mut state, pools);
        let mut input = spec(format!("http://{address}"), 2);
        input.save_to_project = false;
        input.resource_pool = Some(IntruderPoolSelection::Existing(old.id));
        let mut sessions = [admit(&mut state, input.clone()), admit(&mut state, input.clone())];
        sessions.sort();
        let mut project_input = input.clone();
        project_input.save_to_project = true;
        let project_run = admit(&mut state, project_input.clone());
        for id in sessions {
            state.advance_intruder(id).await.unwrap();
        }
        seen.await.unwrap();
        let workspace = state.store.workspace().unwrap();
        let project_before = state.intruder_run(project_run).unwrap();
        let before = sessions.map(|id| state.intruder_run(id).unwrap());
        state.store.fail_session_pool_delete_commit(sessions[1]);
        let error = state
            .delete_intruder_resource_pool(
                state.project,
                Uuid::new_v4(),
                old.id,
                (catalog.revision, workspace.revision),
                true,
                &CancellationToken::new(),
            )
            .unwrap_err();
        assert_eq!(error.reassigned_sessions, 1);
        assert!(!error.default_pool_created);
        assert!(error.message.contains("已改绑 1 个 Session"));
        assert_eq!(error.catalog.as_ref(), Some(&catalog));
        assert_eq!(error.workspace.as_deref(), Some(&workspace));
        assert_eq!(state.intruder.resource_pools, catalog);
        assert_eq!(state.store.intruder_resource_pools().unwrap(), catalog);
        assert_eq!(state.store.workspace().unwrap(), workspace);
        assert_eq!(state.store.read_intruder_run(project_run).unwrap(), project_before);
        assert_eq!(state.intruder.runs[&project_run].summary, project_before);
        let mut expected = before.clone();
        expected[0].resource_pool = Some(default);
        expected[0].resource_pool_revision += 1;
        for (index, id) in sessions.into_iter().enumerate() {
            assert_eq!(state.store.read_intruder_run(id).unwrap(), expected[index]);
            assert_eq!(state.intruder.runs[&id].summary, expected[index]);
            assert!(!state.intruder.runs[&id].cancel.is_cancelled());
            assert_eq!(state.store.read_intruder_spec(id).unwrap(), input);
        }
        assert_eq!(state.intruder.requests.len(), 2);
        assert!(state.intruder.requests.values().all(|request| request.resource_pool == Some(old.id)));
        release.send(()).unwrap();
        finish_next(&mut state).await;
        finish_next(&mut state).await;
        for (index, id) in sessions.into_iter().enumerate() {
            let run = state.intruder_run(id).unwrap();
            assert_eq!((run.resource_pool, run.resource_pool_revision),
                (expected[index].resource_pool, expected[index].resource_pool_revision));
            assert_eq!((run.dispatched, run.completed), (1, 1));
        }
        state.stop_intruder().await.unwrap();
        assert!(!state.intruder.has_resources());
        assert!(state.intruder.requests.is_empty());
        let mut snapshots = Vec::new();
        for (index, id) in sessions.into_iter().enumerate() {
            let page = state.intruder_result_page(id, None, 2).unwrap();
            assert_eq!(page.run.state, IntruderRunState::Stopped);
            assert_eq!((page.run.total, page.run.dispatched, page.run.completed), (2, 1, 1));
            assert_eq!((page.run.resource_pool, page.run.resource_pool_revision),
                (expected[index].resource_pool, expected[index].resource_pool_revision));
            assert!(!page.persisted);
            assert_eq!(page.rows.len(), 1);
            let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else {
                panic!("the completed response must remain inspectable")
            };
            let record = state.read_flow(summary.id).unwrap();
            assert_eq!(record.summary.state, FlowState::Complete);
            assert_eq!(record.request.body, [255]);
            assert_eq!(record.response.as_ref().unwrap().body, b"OK");
            snapshots.push((page, record));
        }
        let project_stopped = state.intruder_run(project_run).unwrap();
        assert_eq!(project_stopped.state, IntruderRunState::Stopped);
        assert_eq!((project_stopped.dispatched, project_stopped.completed), (0, 0));
        assert_eq!((project_stopped.resource_pool, project_stopped.resource_pool_revision),
            (project_before.resource_pool, project_before.resource_pool_revision));
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let first = directory.path().join("partial.mio");
        let copy = directory.path().join("partial-copy.mio");
        save(&mut state, Some(first.clone())).await.unwrap();
        {
            let disk = ProjectStore::open(&first).unwrap();
            assert_eq!(disk.intruder_resource_pools().unwrap(), catalog);
            assert_eq!(disk.workspace().unwrap(), workspace);
            assert_eq!(disk.read_intruder_run(project_run).unwrap(), project_stopped);
            for (page, record) in &snapshots {
                assert!(disk.find_intruder_run(page.run.id).unwrap().is_none());
                assert!(disk.read_optional(record.summary.id).unwrap().is_none());
                assert_eq!(state.intruder_result_page(page.run.id, None, 2).unwrap(), *page);
                assert_eq!(state.read_flow(record.summary.id).unwrap(), *record);
            }
            disk.close().unwrap();
        }
        for (page, _) in &mut snapshots {
            let mut saved_workspace = state.store.workspace().unwrap();
            saved_workspace.revision += 1;
            let promoted = state
                .set_intruder_retention(
                    state.project,
                    page.run.id,
                    page.run.revision,
                    IntruderRetention::Project,
                    saved_workspace,
                )
                .unwrap();
            let mut expected = page.run.clone();
            expected.revision += 1;
            expected.retention = IntruderRetention::Project;
            assert_eq!(promoted.run, expected);
            page.run = promoted.run;
            page.persisted = true;
        }
        save(&mut state, None).await.unwrap();
        save(&mut state, Some(copy.clone())).await.unwrap();
        for path in [&first, &copy] {
            let reopened = ProjectStore::open(path).unwrap();
            assert_eq!(reopened.intruder_resource_pools().unwrap(), catalog);
            assert_eq!(reopened.read_intruder_run(project_run).unwrap(), project_stopped);
            assert_eq!(reopened.read_intruder_spec(project_run).unwrap(), project_input);
            for (page, record) in &snapshots {
                assert_eq!(reopened.read_intruder_run(page.run.id).unwrap(), page.run);
                assert_eq!(reopened.read_intruder_spec(page.run.id).unwrap(), input);
                assert_eq!(reopened.read(record.summary.id).unwrap(), *record);
                assert_eq!(reopened.intruder_results(page.run.id, None, 2).unwrap(), *page);
            }
            reopened.close().unwrap();
        }
        assert_eq!(state.unsaved_count(), 0);
        no_extra(origin.await.unwrap());
    })
    .await
    .expect("two small Session responses survive a partial delete, Stop, promotion and SaveAs without replay");
}
