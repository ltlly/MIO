use super::*;

#[tokio::test]
async fn out_of_band_stop_closes_a_socket_under_ui_backpressure_and_shutdown_saves_its_result() {
    tokio::time::timeout(DEADLINE, async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, received) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let raw = receive_request(&mut stream).await;
            seen.send(body(&raw).to_vec()).unwrap();
            assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
            listener
        });
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = opened(&mut client).await;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("backpressure.mio");
        client.commands.send(Command::SaveProject { websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(), project, path: Some(path.clone()), workspace: None }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::ProjectSaved { .. })).await;
        let input = spec(format!("http://{address}"), vec![b"once".to_vec(), b"never".to_vec()]);
        let cancel = CancellationToken::new();
        let id = Uuid::new_v4();
        let tab_id = bind(&mut client, project, &mut Workspace::default(), &input).await;
        start(&mut client, project, id, tab_id, &input, cancel.clone()).await.unwrap();
        assert_eq!(received.await.unwrap(), b"once");
        let commands = client.commands.clone();
        let flood = tokio::spawn(async move {
            for query_revision in 0..512 {
                commands.send(Command::QueryIntruderRuns { project, query_revision, after: None, limit: 1 }).await.unwrap();
            }
        });
        while client.events.len() != 128 { tokio::task::yield_now().await; }
        // No UI event is drained after saturation; cancellation must reach the socket independently.
        cancel.cancel();
        let listener = origin.await.unwrap();
        flood.abort();
        assert!(flood.await.unwrap_err().is_cancelled());
        shutdown(runtime).await;
        drop(client);
        let (runtime, mut client) = Runtime::start().unwrap();
        opened(&mut client).await;
        client.commands.send(Command::OpenProject { path }).await.unwrap();
        let reopened = opened(&mut client).await;
        let page = results(&mut client, reopened, id, 1, None, 1).await.unwrap();
        assert!(page.persisted);
        assert_eq!(page.run.state, IntruderRunState::Stopped);
        assert_eq!((page.run.total, page.run.dispatched, page.run.completed), (2, 1, 1));
        assert!(matches!(page.rows.as_slice(), [row]
            if matches!(&row.outcome, IntruderResultOutcome::Flow(flow) if flow.state == FlowState::Cancelled)));
        shutdown(runtime).await;
        no_extra_connection(listener);
    }).await.expect("full UI queue must not prevent real socket cancellation or durable shutdown");
}
