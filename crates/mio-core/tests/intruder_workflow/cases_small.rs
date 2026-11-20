use super::*;

#[tokio::test]
async fn empty_payload_set_is_saveable_but_start_is_rejected_without_network_io() {
    tokio::time::timeout(DEADLINE, async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = opened(&mut client).await;
        let spec = spec(format!("http://{}", listener.local_addr().unwrap()), vec![]);
        let run_id = Uuid::new_v4();
        let tab_id = bind(&mut client, project, &mut Workspace::default(), &spec).await;
        let error = start(
            &mut client,
            project,
            run_id,
            tab_id,
            &spec,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(error.contains("payload 列表为空"));
        assert!(
            results(&mut client, project, run_id, 1, None, 1)
                .await
                .is_err()
        );
        shutdown(runtime).await;
        no_extra_connection(listener);
    })
    .await
    .expect("empty Intruder input must remain a draft without network activity");
}
