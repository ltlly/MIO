use super::*;

#[tokio::test]
async fn repeater_sse_streaming_progress_and_cancel_preserve_partial_response_across_save_as_and_stop()
 {
    tokio::time::timeout(Duration::from_secs(8), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (chunk_arrived_tx, chunk_arrived_rx) = oneshot::channel();
        let (hold_tx, hold_rx) = oneshot::channel::<()>();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                head.push(stream.read_u8().await.unwrap());
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n",
                )
                .await
                .unwrap();
            stream
                .write_all(b"13\r\ndata: first-event\n\n\r\n")
                .await
                .unwrap();
            stream.flush().await.unwrap();
            let _ = chunk_arrived_tx.send(());
            let _ = hold_rx.await;
            listener.into_std().unwrap()
        });

        let (events, mut receiver) = mpsc::channel(64);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let mut request = tests::completed_flow().request;
        request.method = "GET".into();
        request.url = format!("http://{address}/sse");
        request.body.clear();
        let mut workspace = state.store.workspace().unwrap();
        let tab_id = workspace.open_request(Some(&request)).unwrap();
        let draft = workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .unwrap()
            .draft
            .clone();
        state
            .command(Command::SaveWorkspace {
                project: state.project,
                workspace,
            })
            .await
            .unwrap();
        let run_id = Uuid::new_v4();
        state
            .command(Command::Send {
                project: state.project,
                tab_id,
                run_id,
                draft: draft.clone(),
                settings: RepeaterSettings::default(),
                group_send: None,
                cancel: CancellationToken::new(),
            })
            .await
            .unwrap();

        chunk_arrived_rx.await.unwrap();
        loop {
            let progressed_tab = next_repeater_progress(&mut state.sends, &state.events, true).await;
            assert_eq!(progressed_tab, tab_id);
            state.emit_repeater_progress(progressed_tab);
            let events_so_far = drain_events(&mut receiver);
            if events_so_far.iter().any(|event| {
                matches!(
                    event,
                    Event::SendProgress {
                        tab_id: actual_tab,
                        run_id: actual_run,
                        draft_revision,
                        response,
                        ..
                    } if *actual_tab == tab_id
                        && *actual_run == run_id
                        && *draft_revision == draft.revision
                        && response.status == 200
                        && response.body.starts_with(b"data: first-event\n\n")
                )
            }) {
                break;
            }
        }

        state.store.reject_writes(true);
        state
            .command(Command::CancelSend {
                project: state.project,
                tab_id,
                run_id,
                draft_revision: draft.revision,
            })
            .await
            .unwrap();
        assert!(state.sends.get(&tab_id).unwrap().progress_rx.is_none());

        let stop_res = state
            .stop_repeater_until(tokio::time::Instant::now() + Duration::from_secs(3))
            .await;
        assert!(stop_res.is_err(), "reject_writes forces recovery queue on completion");
        assert!(state.jobs.is_empty() && state.sends.is_empty());
        let _ = hold_tx.send(());
        let _listener = origin.await.unwrap();

        let finished_events = drain_events(&mut receiver);
        let partial_record = finished_events
            .into_iter()
            .find_map(|event| match event {
                Event::SendFinished {
                    tab_id: actual_tab,
                    record,
                    persisted: false,
                    ..
                } if actual_tab == tab_id => Some(*record),
                _ => None,
            })
            .expect("cancelled stream must emit SendFinished with partial response");
        assert_eq!(partial_record.summary.id, run_id);
        assert_eq!(partial_record.summary.revision, 2);
        assert_eq!(partial_record.summary.state, FlowState::Cancelled);
        assert_eq!(partial_record.summary.status, Some(200));
        assert_eq!(
            partial_record.response.as_ref().unwrap().body,
            b"data: first-event\n\n"
        );
        assert_eq!(
            partial_record.summary.response_bytes,
            b"data: first-event\n\n".len() as u64
        );

        let directory = tempfile::tempdir().unwrap();
        let save_as_path = directory.path().join("sse-partial.mio");
        state
            .command(Command::SaveProject {
                project: state.project,
                path: Some(save_as_path.clone()),
                workspace: None,
                websocket_notes: Vec::new(),
                ext_http_notes: Vec::new(),
                ext_http_highlights: Vec::new(),
            })
            .await
            .unwrap();
        assert_eq!(state.unsaved_count(), 0);
        state.store.close().unwrap();

        let reopened = ProjectStore::open(&save_as_path).unwrap();
        assert_eq!(reopened.read(run_id).unwrap(), partial_record);
        let history_page = reopened.repeater_history_page(tab_id, 0, 10).unwrap();
        assert_eq!(history_page.rows.len(), 1);
        assert_eq!(history_page.rows[0].status, Some(200));
        reopened.close().unwrap();
    })
    .await
    .expect("SSE streaming progress, cancel, recovery, SaveAs and reopen stay bounded");
}
