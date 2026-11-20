use super::*;

#[tokio::test]
async fn custom_send_timeout_triggers_timeout_error_and_closes_socket() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                assert!(head.len() < 1024);
                head.push(stream.read_u8().await.unwrap());
            }
            assert!(head.starts_with(b"POST /slow HTTP/1.1\r\n"));
            let mut body = [0; 4];
            stream.read_exact(&mut body).await.unwrap();
            assert_eq!(&body, b"ping");
            assert_eq!(
                stream.read(&mut [0]).await.unwrap(),
                0,
                "timeout cleanup must close the origin socket before returning"
            );
            listener
        });

        let cancel = CancellationToken::new();
        let prepared = prepare_send(
            &HttpRequest {
                method: "POST".into(),
                url: format!("http://{address}/slow"),
                version: HttpVersion::Http11,
                headers: Vec::new(),
                body: b"ping".to_vec(),
                trailers: Vec::new(),
            },
            SendConfig {
                timeout: Duration::from_millis(80),
                http_version: Some(HttpVersion::Http11),
                update_content_length: false,
                ..SendConfig::default()
            },
        )
        .unwrap();

        let result = prepared.send_with_metrics(cancel.clone()).await;
        assert!(
            matches!(result, Err(ProxyError::Timeout)),
            "expected ProxyError::Timeout, got {result:?}"
        );
        assert!(
            !cancel.is_cancelled(),
            "timeout cleanup cancels child token, not caller token"
        );
        let listener = origin.await.unwrap();
        assert_eq!(
            listener.into_std().unwrap().accept().unwrap_err().kind(),
            ErrorKind::WouldBlock,
            "timed out send must never retry"
        );
    })
    .await
    .expect("timeout test completes within bounded fixture");
}

#[tokio::test]
async fn send_streaming_emits_head_incremental_chunks_and_trailers_before_eof() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (step1_tx, step1_rx) = oneshot::channel();
        let (step2_tx, step2_rx) = oneshot::channel();
        let (step3_tx, step3_rx) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                assert!(head.len() < 1024);
                head.push(stream.read_u8().await.unwrap());
            }
            assert!(head.starts_with(b"GET /sse HTTP/1.1\r\n"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nTrailer: x-end\r\n\r\n",
                )
                .await
                .unwrap();
            step1_rx.await.unwrap();
            stream.write_all(b"6\r\ndata:1\r\n").await.unwrap();
            step2_rx.await.unwrap();
            stream.write_all(b"6\r\n\ndata:\r\n").await.unwrap();
            step3_rx.await.unwrap();
            stream
                .write_all(b"0\r\nx-end: done\r\nx-end: second\r\n\r\n")
                .await
                .unwrap();
            assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
            listener
        });

        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(send_streaming(
            HttpRequest {
                method: "GET".into(),
                url: format!("http://{address}/sse"),
                version: HttpVersion::Http11,
                headers: Vec::new(),
                body: Vec::new(),
                trailers: Vec::new(),
            },
            SendConfig::default(),
            cancel.clone(),
            move |response| {
                progress_tx.send(response.clone()).unwrap();
            },
        ));

        let head_progress = progress_rx.recv().await.unwrap();
        assert_eq!(head_progress.status, 200);
        assert!(head_progress.body.is_empty());
        assert!(head_progress.trailers.is_empty());
        step1_tx.send(()).unwrap();

        let first_chunk = progress_rx.recv().await.unwrap();
        assert_eq!(first_chunk.body, b"data:1");
        assert!(first_chunk.trailers.is_empty());
        step2_tx.send(()).unwrap();

        let second_chunk = progress_rx.recv().await.unwrap();
        assert_eq!(second_chunk.body, b"data:1\ndata:");
        assert!(second_chunk.trailers.is_empty());
        step3_tx.send(()).unwrap();

        let with_trailers = progress_rx.recv().await.unwrap();
        assert_eq!(with_trailers.body, b"data:1\ndata:");
        assert_eq!(
            with_trailers.trailers,
            vec![
                Header {
                    name: "x-end".into(),
                    value: b"done".to_vec(),
                },
                Header {
                    name: "x-end".into(),
                    value: b"second".to_vec(),
                },
            ]
        );

        let outcome = task.await.unwrap();
        assert!(outcome.error.is_none());
        assert_eq!(outcome.response.as_ref(), Some(&with_trailers));
        assert!(!cancel.is_cancelled());
        assert_eq!(
            origin
                .await
                .unwrap()
                .into_std()
                .unwrap()
                .accept()
                .unwrap_err()
                .kind(),
            ErrorKind::WouldBlock
        );
    })
    .await
    .expect("streaming head, chunks, and trailers complete within bounded fixture");
}

#[tokio::test]
async fn send_streaming_preserves_partial_bytes_and_closes_socket_on_cancel_timeout_and_body_limit()
{
    tokio::time::timeout(Duration::from_secs(5), async {
        for mode in ["cancel", "timeout", "limit"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (second_tx, second_rx) = oneshot::channel();
            let origin = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    assert!(head.len() < 1024);
                    head.push(stream.read_u8().await.unwrap());
                }
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n",
                    )
                    .await
                    .unwrap();
                if second_rx.await.is_ok() {
                    stream.write_all(b"4\r\nefgh\r\n").await.unwrap();
                }
                assert_eq!(
                    stream.read(&mut [0]).await.unwrap(),
                    0,
                    "mode {mode} must close socket and join SDK workers before returning"
                );
                listener
            });

            let (first_seen_tx, first_seen_rx) = oneshot::channel();
            let mut first_seen_tx = Some(first_seen_tx);
            let cancel = CancellationToken::new();
            let task_cancel = cancel.clone();
            let config = SendConfig {
                timeout: if mode == "timeout" {
                    Duration::from_millis(120)
                } else {
                    Duration::from_secs(4)
                },
                max_body_bytes: if mode == "limit" { 6 } else { 1024 },
                ..SendConfig::default()
            };
            let mut snapshots = Vec::new();
            let task = tokio::spawn(async move {
                let outcome = send_streaming(
                    HttpRequest {
                        method: "GET".into(),
                        url: format!("http://{address}/stream-{mode}"),
                        version: HttpVersion::Http11,
                        headers: Vec::new(),
                        body: Vec::new(),
                        trailers: Vec::new(),
                    },
                    config,
                    task_cancel,
                    |response| {
                        snapshots.push(response.body.clone());
                        if response.body == b"abcd"
                            && let Some(tx) = first_seen_tx.take()
                        {
                            let _ = tx.send(());
                        }
                    },
                )
                .await;
                (outcome, snapshots)
            });

            first_seen_rx.await.unwrap();
            match mode {
                "cancel" => {
                    drop(second_tx);
                    cancel.cancel();
                }
                "limit" => second_tx.send(()).unwrap(),
                "timeout" => {
                    drop(second_tx);
                }
                _ => unreachable!(),
            }

            let (outcome, snapshots) = task.await.unwrap();
            let response = outcome.response.expect("partial response retained");
            match mode {
                "cancel" => {
                    assert!(matches!(outcome.error, Some(ProxyError::Cancelled)));
                    assert_eq!(response.body, b"abcd");
                    assert_eq!(snapshots, vec![b"".to_vec(), b"abcd".to_vec()]);
                }
                "timeout" => {
                    assert!(matches!(outcome.error, Some(ProxyError::Timeout)));
                    assert_eq!(response.body, b"abcd");
                    assert_eq!(snapshots, vec![b"".to_vec(), b"abcd".to_vec()]);
                }
                "limit" => {
                    assert!(matches!(
                        outcome.error,
                        Some(ProxyError::BodyLimit { limit: 6 })
                    ));
                    assert_eq!(response.body, b"abcdef");
                    assert_eq!(
                        snapshots,
                        vec![b"".to_vec(), b"abcd".to_vec(), b"abcdef".to_vec()]
                    );
                }
                _ => unreachable!(),
            }
            assert_eq!(
                origin
                    .await
                    .unwrap()
                    .into_std()
                    .unwrap()
                    .accept()
                    .unwrap_err()
                    .kind(),
                ErrorKind::WouldBlock
            );
        }
    })
    .await
    .expect("cancel, timeout, and body limit streaming checks are bounded");
}
