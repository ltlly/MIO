use super::*;

#[tokio::test]
async fn start_project_with_pause_automated_tasks_keeps_unfinished_runs_paused_until_resumed() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let path = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("paused-start-project.mio");
        let mut store = ProjectStore::create(Some(&path)).unwrap();
        let mut input = spec(format!("http://{address}"), 2);
        input.payload_sets = vec![vec![vec![b'A'], vec![b'B']]];
        let tab = bind(&mut store, &input);
        let id = Uuid::new_v4();
        store.create_intruder_run(id, tab, &input, 1).unwrap();
        store.close().unwrap();

        let (mut state, mut events) = state();
        state
            .command(Command::StartProject {
                path: Some(path.clone()),
                create_new: false,
                options: ProjectStartOptions {
                    name: Some("恢复暂停项目".into()),
                    config_mode: StartupDefaultConfig::Project,
                    pause_automated_tasks: true,
                    trust_project_file: true,
                    disable_extensions: true,
                },
            })
            .await
            .unwrap();
        let project = state.project;
        assert!(state.extensions_disabled);
        let restored = state.intruder_result_page(id, None, 1).unwrap();
        assert_eq!(
            (
                restored.run.state,
                restored.run.dispatched,
                restored.run.completed
            ),
            (IntruderRunState::Paused, 0, 0)
        );
        assert!(!state.intruder.has_resources());

        let resumed = control(&mut state, &mut events, project, id, false)
            .await
            .unwrap();
        assert_eq!(resumed.state, IntruderRunState::Running);
        assert!(state.intruder.has_resources());

        state
            .command(Command::StopIntruder {
                project,
                run_id: id,
            })
            .await
            .unwrap();
        state.advance_intruder(id).await.unwrap();
        state.advance_intruder(id).await.unwrap();
        assert_eq!(
            state.intruder_run(id).unwrap().state,
            IntruderRunState::Stopped
        );
        assert_eq!(
            listener.into_std().unwrap().accept().unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
    })
    .await
    .expect("StartProject with pause_automated_tasks keeps unfinished runs paused without auto-dispatch");
}

#[tokio::test]
async fn out_of_band_stop_wakes_an_idle_paused_run_with_a_full_event_queue() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let (mut state, mut events) = state();
        let project = state.project;
        let id = admit(&mut state, spec("http://127.0.0.1:1".into(), 1));
        control(&mut state, &mut events, project, id, true).await.unwrap();
        state.intruder.queried();
        while state.events.try_send(Event::IntruderChanged { project }).is_ok() {}
        let cancel = state.intruder.runs[&id].cancel.clone();
        let (polled, ready) = oneshot::channel();
        let trigger = tokio::spawn(async move {
            ready.await.unwrap();
            cancel.cancel();
        });
        let signal = {
            let mut waiting = std::pin::pin!(state.intruder.next_signal(&state.events, false));
            let mut polled = Some(polled);
            tokio::time::timeout(Duration::from_secs(1), std::future::poll_fn(|cx| {
                let state = waiting.as_mut().poll(cx);
                if state.is_pending()
                    && let Some(polled) = polled.take()
                {
                    polled.send(()).unwrap();
                }
                state
            })).await.expect("a registered cancellation must wake idle Intruder even without a queued Stop command")
        };
        trigger.await.unwrap();
        assert!(matches!(signal, Signal::Advance(run_id) if run_id == id));
        state.intruder_signal(signal).await.unwrap();
        state.advance_intruder(id).await.unwrap();
        assert!(!state.intruder.has_resources());
        assert_eq!(state.intruder_run(id).unwrap().state, IntruderRunState::Stopped);
    }).await.expect("idle cancellation wakeup and cleanup remain bounded without sleeps");
}
