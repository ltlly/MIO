use super::*;

async fn delayed_sdk_worker(cancel_after_post: bool) -> Result<HttpResponse, ProxyError> {
    tokio::time::timeout(Duration::from_secs(7), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (received, post_received) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).await.unwrap();
                head.push(byte[0]);
                assert!(head.len() <= 1024, "bounded synthetic request head");
            }
            assert!(head.starts_with(b"POST /owned-send HTTP/1.1\r\n"));
            assert!(
                String::from_utf8(head)
                    .unwrap()
                    .to_ascii_lowercase()
                    .contains("\r\ncontent-length: 3\r\n")
            );
            let mut body = [0; 3];
            stream.read_exact(&mut body).await.unwrap();
            assert_eq!(body, PAYLOAD);
            received.send(()).unwrap();
            if !cancel_after_post {
                stream
                    .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 3\r\n\r\n")
                    .await
                    .unwrap();
                stream.write_all(RESPONSE).await.unwrap();
            }
            assert_eq!(
                stream.read(&mut [0]).await.unwrap(),
                0,
                "cleanup closes the real POST socket before returning"
            );
            listener
        });

        let cancel = CancellationToken::new();
        let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
        let (release, released) = oneshot::channel();
        let (stopping, worker_stopping) = oneshot::channel();
        let worker_cancel = cancel.clone();
        let worker = Executor::graceful(shutdown.guard()).into_spawn_task(async move {
            worker_cancel.cancelled().await;
            stopping.send(()).unwrap();
            released.await.unwrap();
        });
        let mut sending = tokio::spawn(send_owned(
            SendInput::Message(HttpRequest {
                method: "POST".into(),
                url: format!("http://{address}/owned-send"),
                version: HttpVersion::Http11,
                headers: Vec::new(),
                body: PAYLOAD.to_vec(),
                trailers: Vec::new(),
            }),
            SendConfig {
                timeout: Duration::from_secs(4),
                http_version: Some(HttpVersion::Http11),
                ..SendConfig::default()
            },
            cancel.clone(),
            shutdown,
        ));
        post_received.await.unwrap();
        if cancel_after_post {
            cancel.cancel();
        }
        worker_stopping.await.unwrap();

        // The SDK worker stays causally held across the old two-second cleanup deadline.
        let early = tokio::time::timeout(Duration::from_secs(3), &mut sending)
            .await
            .ok();
        let returned_early = early.is_some();
        let worker_finished = worker.is_finished();
        release.send(()).unwrap();
        worker.await.unwrap();
        let result = match early {
            Some(result) => result.unwrap(),
            None => sending.await.unwrap(),
        };
        let listener = origin.await.unwrap();
        assert_eq!(
            listener.into_std().unwrap().accept().unwrap_err().kind(),
            ErrorKind::WouldBlock,
            "neither cancellation nor delayed cleanup may retry the POST"
        );
        assert!(
            !worker_finished,
            "the fixture must still own its SDK worker"
        );
        assert!(
            !returned_early,
            "send returned {result:?} while its SDK worker was still held"
        );
        result
    })
    .await
    .expect("SDK drain, real socket EOF and all fixture joins are bounded")
}

#[tokio::test]
async fn successful_send_waits_for_sdk_workers_without_losing_response() {
    let response = delayed_sdk_worker(false).await.unwrap();
    assert_eq!(response.status, 201);
    assert_eq!(response.body, RESPONSE);
}

#[tokio::test]
async fn cancelled_send_waits_for_sdk_workers_and_never_retries_post() {
    assert!(matches!(
        delayed_sdk_worker(true).await,
        Err(ProxyError::Cancelled)
    ));
}

#[tokio::test]
async fn prepared_send_freezes_the_validated_message_without_opening_a_socket() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut request = HttpRequest {
            method: "POST".into(),
            url: format!("http://{}/prepared", listener.local_addr().unwrap()),
            version: HttpVersion::Http11,
            headers: Vec::new(),
            body: PAYLOAD.to_vec(),
            trailers: Vec::new(),
        };
        let prepared = prepare_send(&request, SendConfig::default()).unwrap();
        request.method = "CONNECT".into();
        request.url = "invalid after preparation".into();
        request.body.fill(17);
        drop(request);
        let listener = listener.into_std().unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            ErrorKind::WouldBlock,
            "preparation performs no connection attempt"
        );
        let listener = TcpListener::from_std(listener).unwrap();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                assert!(head.len() < 1024);
                head.push(stream.read_u8().await.unwrap());
            }
            assert!(head.starts_with(b"POST /prepared HTTP/1.1\r\n"));
            let mut body = [0; 3];
            stream.read_exact(&mut body).await.unwrap();
            assert_eq!(body, PAYLOAD);
            stream
                .write_all(
                    b"HTTP/1.1 201 Created\r\nContent-Length: 3\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            stream.write_all(RESPONSE).await.unwrap();
            assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
            listener
        });
        let cancel = CancellationToken::new();
        let response = prepared.send(cancel.clone()).await.unwrap();
        assert_eq!(response.status, 201);
        assert_eq!(response.body, RESPONSE);
        assert!(
            !cancel.is_cancelled(),
            "SDK cleanup cancels its child, not the owner's token"
        );
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
    .expect("a prepared binary POST sends once and normally drains its socket");
}

#[tokio::test]
async fn preparation_rejects_local_errors_and_legacy_send_keeps_cancellation_priority() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let request = HttpRequest {
            method: "POST".into(),
            url: format!("http://{}/prepared", listener.local_addr().unwrap()),
            version: HttpVersion::Http11,
            headers: Vec::new(),
            body: PAYLOAD.to_vec(),
            trailers: Vec::new(),
        };
        let mut connect = request.clone();
        connect.method = "CONNECT".into();
        assert!(matches!(prepare_send(&connect, SendConfig::default()), Err(ProxyError::InvalidMessage(message)) if message.contains("CONNECT")));
        let mut backslash = request.clone();
        backslash.url.push_str("\\raw");
        assert!(matches!(prepare_send(&backslash, SendConfig::default()), Err(ProxyError::InvalidMessage(message)) if message.contains("SDK")));
        assert!(matches!(prepare_send(&request, SendConfig { max_body_bytes: 2, ..SendConfig::default() }), Err(ProxyError::BodyLimit { limit: 2 })));
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(send(connect, SendConfig::default(), cancel.clone()).await, Err(ProxyError::Cancelled)), "an already cancelled legacy send must not change into a validation failure");
        let prepared = prepare_send(&request, SendConfig::default()).unwrap();
        assert!(matches!(prepared.send(cancel).await, Err(ProxyError::Cancelled)));
        assert_eq!(listener.into_std().unwrap().accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    }).await.expect("local rejection and both cancelled entry points create no socket");
}

#[tokio::test]
async fn manual_response_metadata_is_bounded_before_body_collection() {
    use rama::http::{HeaderMap, HeaderValue};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let polled = Arc::new(AtomicBool::new(false));
    let observed = polled.clone();
    let body = Body::from_stream(rama::futures::stream::poll_fn(move |_| {
        observed.store(true, Ordering::SeqCst);
        std::task::Poll::Ready(None::<Result<Vec<u8>, std::io::Error>>)
    }));
    let mut response = Response::new(body);
    response.headers_mut().insert(
        "x-large",
        HeaderValue::from_bytes(&vec![b'a'; 128 * 1024]).unwrap(),
    );
    let result = collect_response(response, 64).await;
    assert!(
        matches!(result, Err(ProxyError::InvalidMessage(_))),
        "oversized response metadata must be rejected"
    );
    assert!(
        !polled.load(Ordering::SeqCst),
        "head budget is checked before polling the body"
    );

    let mut trailers = HeaderMap::new();
    for _ in 0..1024 {
        trailers.append("x-end", HeaderValue::from_static("a"));
    }
    let mut response = Response::new(
        Body::from_stream(rama::futures::stream::iter([Ok::<_, std::io::Error>(
            vec![0, 255],
        )]))
        .with_trailer_headers(trailers),
    );
    response
        .headers_mut()
        .insert("x-head", HeaderValue::from_static("a"));
    assert!(
        matches!(
            collect_response(response, 64).await,
            Err(ProxyError::InvalidMessage(_))
        ),
        "head and trailers share one metadata budget"
    );

    let mut response = Response::new(Body::from(vec![0, 255]));
    response.headers_mut().insert(
        "x",
        HeaderValue::from_bytes(&vec![b'a'; 128 * 1024 - 5]).unwrap(),
    );
    let exact = collect_response(response, 64).await.unwrap();
    assert_eq!(exact.body, [0, 255]);
    assert_eq!(exact.headers[0].value.len(), 128 * 1024 - 5);
    let mut trailers = HeaderMap::new();
    for _ in 0..1023 {
        trailers.append("x-end", HeaderValue::from_static("a"));
    }
    let mut response = Response::new(
        Body::from_stream(rama::futures::stream::iter([Ok::<_, std::io::Error>(
            vec![0, 255],
        )]))
        .with_trailer_headers(trailers),
    );
    response
        .headers_mut()
        .insert("x-head", HeaderValue::from_static("b"));
    let exact = collect_response(response, 64).await.unwrap();
    assert_eq!(exact.body, [0, 255]);
    assert_eq!(exact.headers[0].value, b"b");
    assert_eq!(exact.trailers.len(), 1023);
    assert!(
        exact
            .trailers
            .iter()
            .all(|header| header.name == "x-end" && header.value == b"a")
    );
}

#[tokio::test]
async fn send_with_metrics_measures_first_byte_before_the_delayed_tail() {
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
            assert!(head.starts_with(b"GET /metrics HTTP/1.1\r\n"));
            stream
                .write_all(b"HTTP/1.1 200 MIO nonstandard reason\r\n")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
            stream
                .write_all(b"Content-Length: 3\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            stream.write_all(RESPONSE).await.unwrap();
            assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
            listener
        });

        let prepared = prepare_send(
            &HttpRequest {
                method: "GET".into(),
                url: format!("http://{address}/metrics"),
                version: HttpVersion::Http11,
                headers: Vec::new(),
                body: Vec::new(),
                trailers: Vec::new(),
            },
            SendConfig::default(),
        )
        .unwrap();

        let cancel = CancellationToken::new();
        let started = Instant::now();
        let metrics = prepared.send_with_metrics(cancel.clone()).await.unwrap();
        let total_ms = started.elapsed().as_millis();
        assert_eq!(metrics.response.status, 200);
        assert_eq!(metrics.response.body, RESPONSE);
        assert_eq!(
            metrics.reason_phrase.as_deref(),
            Some(b"MIO nonstandard reason".as_slice())
        );
        assert!(
            metrics.response_received_ms < 150,
            "first byte must precede the delayed tail, got {} ms",
            metrics.response_received_ms
        );
        assert!(
            total_ms >= 200,
            "the delayed tail must be included in the total, got {total_ms} ms"
        );
        assert!(
            !cancel.is_cancelled(),
            "SDK cleanup cancels its child, not the owner's token"
        );
        origin.await.unwrap();
    })
    .await
    .expect("metrics send completes within the bounded fixture");
}
