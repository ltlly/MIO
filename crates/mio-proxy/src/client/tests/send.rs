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
        let mut owned_cfg = SendConfig::default();
        owned_cfg.limits.timeout = Duration::from_secs(4);
        owned_cfg.http.version = Some(HttpVersion::Http11);
        let mut sending = tokio::spawn(send_owned(
            SendInput::Message(HttpRequest {
                method: "POST".into(),
                url: format!("http://{address}/owned-send"),
                version: HttpVersion::Http11,
                headers: Vec::new(),
                body: PAYLOAD.to_vec(),
                trailers: Vec::new(),
            }),
            owned_cfg,
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
        let mut small_limit = SendConfig::default();
        small_limit.limits.max_body_bytes = 2;
        assert!(matches!(prepare_send(&request, small_limit), Err(ProxyError::BodyLimit { limit: 2 })));
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
            metrics.response.reason_phrase.as_deref(),
            Some(b"MIO nonstandard reason".as_slice())
        );
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

static POOL_CACHE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn read_h1_request_head(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = stream.read(&mut byte).await.ok()?;
        if n == 0 {
            return None;
        }
        head.push(byte[0]);
        assert!(head.len() <= 8192, "bounded synthetic request head");
    }
    Some(String::from_utf8(head).expect("utf-8 request head"))
}

fn synthetic_ntlm_type2_header() -> String {
    use crate::auth::{
        NTLMSSP_NEGOTIATE_EXTENDED_SESSIONSECURITY, NTLMSSP_NEGOTIATE_NTLM,
        NTLMSSP_NEGOTIATE_UNICODE,
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    let mut raw = Vec::new();
    raw.extend_from_slice(b"NTLMSSP\0");
    raw.extend_from_slice(&2u32.to_le_bytes());
    raw.extend_from_slice(&0u16.to_le_bytes());
    raw.extend_from_slice(&0u16.to_le_bytes());
    raw.extend_from_slice(&48u32.to_le_bytes());
    let flags = NTLMSSP_NEGOTIATE_UNICODE
        | NTLMSSP_NEGOTIATE_NTLM
        | NTLMSSP_NEGOTIATE_EXTENDED_SESSIONSECURITY;
    raw.extend_from_slice(&flags.to_le_bytes());
    raw.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    raw.extend_from_slice(&[0u8; 16]);
    format!("NTLM {}", BASE64.encode(raw))
}

#[tokio::test]
async fn ntlm_handshake_reuses_ephemeral_connection_without_polluting_global_pool() {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    let _pool_guard = POOL_CACHE_TEST_LOCK.lock().await;
    let pool_len_before = crate::client::exchange::pooled_client_cache_len();

    tokio::time::timeout(Duration::from_secs(7), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            // Initial unauthenticated request returns 401 WWW-Authenticate: NTLM.
            let (mut initial_stream, _) = listener.accept().await.unwrap();
            let first = read_h1_request_head(&mut initial_stream).await.unwrap();
            assert!(!first.to_ascii_lowercase().contains("authorization:"));
            initial_stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: NTLM\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();

            // Type 1 -> Type 2 -> Type 3 MUST reuse one single ephemeral TCP socket.
            let (mut auth_stream, _) = listener.accept().await.unwrap();
            let type1_head = read_h1_request_head(&mut auth_stream).await.unwrap();
            let type1_b64 = type1_head
                .lines()
                .find_map(|line| line.strip_prefix("authorization: NTLM ").or_else(|| line.strip_prefix("Authorization: NTLM ")))
                .expect("Type 1 Authorization header");
            let type1_raw = BASE64.decode(type1_b64.trim()).unwrap();
            assert_eq!(u32::from_le_bytes(type1_raw[8..12].try_into().unwrap()), 1);

            let challenge_hdr = synthetic_ntlm_type2_header();
            let type2_resp = format!(
                "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: {challenge_hdr}\r\nContent-Length: 0\r\n\r\n"
            );
            auth_stream.write_all(type2_resp.as_bytes()).await.unwrap();
            auth_stream.flush().await.unwrap();

            let type3_head = read_h1_request_head(&mut auth_stream)
                .await
                .expect("Type 3 must arrive on the same TCP connection");
            let type3_b64 = type3_head
                .lines()
                .find_map(|line| line.strip_prefix("authorization: NTLM ").or_else(|| line.strip_prefix("Authorization: NTLM ")))
                .expect("Type 3 Authorization header");
            let type3_raw = BASE64.decode(type3_b64.trim()).unwrap();
            assert_eq!(u32::from_le_bytes(type3_raw[8..12].try_into().unwrap()), 3);

            auth_stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nntlm-ok")
                .await
                .unwrap();
        });

        let mut config = SendConfig::default();
        config.auth = crate::AuthConfig {
            platform_enabled: true,
            platform_rules: std::sync::Arc::from([crate::PlatformAuthRuleConfig {
                enabled: true,
                destination_host: "*".into(),
                credentials: crate::AuthCredentials {
                    scheme: crate::HttpAuthScheme::NtlmV2,
                    username: "user".into(),
                    password: "secret".into(),
                    domain: "LAB".into(),
                    workstation: "WS01".into(),
                },
            }]),
            upstream_rules: std::sync::Arc::from([]),
        };

        let response = send(
            HttpRequest {
                method: "GET".into(),
                url: format!("http://{address}/ntlm"),
                version: HttpVersion::Http11,
                headers: Vec::new(),
                body: Vec::new(),
                trailers: Vec::new(),
            },
            config,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"ntlm-ok");
        origin.await.unwrap();
    })
    .await
    .expect("NTLM handshake completes promptly");

    assert_eq!(
        crate::client::exchange::pooled_client_cache_len(),
        pool_len_before,
        "NTLM ephemeral connections must not pollute the global pooled client cache"
    );
}

#[tokio::test]
async fn pooled_send_records_first_response_byte_on_fresh_and_reused_connections() {
    let _pool_guard = POOL_CACHE_TEST_LOCK.lock().await;

    tokio::time::timeout(Duration::from_secs(7), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (done_tx, done_rx) = oneshot::channel::<()>();

        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            for _attempt in 0..2 {
                let head = read_h1_request_head(&mut stream)
                    .await
                    .expect("pooled sends must reuse one persistent connection");
                assert!(head.starts_with("GET /pooled HTTP/1.1\r\n"));
                stream
                    .write_all(
                        b"HTTP/1.1 200 Pooled Custom\r\nContent-Length: 3\r\nConnection: keep-alive\r\n\r\n",
                    )
                    .await
                    .unwrap();
                stream.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(200)).await;
                stream.write_all(RESPONSE).await.unwrap();
                stream.flush().await.unwrap();
            }
            let _ = done_rx.await;
        });

        let mut config = SendConfig::default();
        config.http.version = Some(HttpVersion::Http11);
        config.http.http1_keep_alive = true;
        config.http.use_connection_pool = true;
        config.group.connection_pool_group_id = Some(0xDEAD_BEEF_CAFE_0001_u128);

        let req = HttpRequest {
            method: "GET".into(),
            url: format!("http://{address}/pooled"),
            version: HttpVersion::Http11,
            headers: Vec::new(),
            body: Vec::new(),
            trailers: Vec::new(),
        };

        for attempt in 0..2 {
            let prepared = prepare_send(&req, config.clone()).unwrap();
            let started = Instant::now();
            let metrics = prepared
                .send_with_metrics(CancellationToken::new())
                .await
                .unwrap();
            let total_ms = started.elapsed().as_millis();
            assert_eq!(metrics.response.status, 200);
            assert_eq!(metrics.response.body, RESPONSE);
            assert_eq!(
                metrics.response.reason_phrase.as_deref(),
                Some(b"Pooled Custom".as_slice())
            );
            assert_eq!(
                metrics.reason_phrase.as_deref(),
                Some(b"Pooled Custom".as_slice())
            );
            assert!(
                metrics.response_received_ms < 150,
                "attempt {attempt}: first byte must precede the delayed body, got {} ms",
                metrics.response_received_ms
            );
            assert!(
                total_ms >= 200,
                "attempt {attempt}: delayed body must be included in total time, got {total_ms} ms"
            );
        }

        let _ = done_tx.send(());
        origin.await.unwrap();
    })
    .await
    .expect("pooled metrics sends complete within timeout");
}

#[tokio::test]
async fn prepared_send_applies_platform_and_upstream_auth_consistently() {
    tokio::time::timeout(Duration::from_secs(7), async {
        // 1. Preemptive Basic platform auth via PreparedSend::send.
        {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let origin = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let head = read_h1_request_head(&mut stream).await.unwrap();
                assert!(
                    head.to_ascii_lowercase().contains("authorization: basic "),
                    "expected preemptive Basic header, got: {head}"
                );
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nbasic-ok")
                    .await
                    .unwrap();
            });
            let mut config = SendConfig::default();
            config.auth = crate::AuthConfig {
                platform_enabled: true,
                platform_rules: std::sync::Arc::from([crate::PlatformAuthRuleConfig {
                    enabled: true,
                    destination_host: "*".into(),
                    credentials: crate::AuthCredentials {
                        scheme: crate::HttpAuthScheme::Basic,
                        username: "alice".into(),
                        password: "secret".into(),
                        ..Default::default()
                    },
                }]),
                upstream_rules: std::sync::Arc::from([]),
            };
            let req = HttpRequest {
                method: "GET".into(),
                url: format!("http://{address}/basic"),
                version: HttpVersion::Http11,
                headers: Vec::new(),
                body: Vec::new(),
                trailers: Vec::new(),
            };
            let resp = prepare_send(&req, config)
                .unwrap()
                .send(CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(resp.status, 200);
            assert_eq!(resp.body, b"basic-ok");
            origin.await.unwrap();
        }

        // 2. 401 Digest challenge/retry via PreparedSend::send_with_metrics.
        {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let origin = tokio::spawn(async move {
                let (mut first, _) = listener.accept().await.unwrap();
                let req1 = read_h1_request_head(&mut first).await.unwrap();
                assert!(!req1.to_ascii_lowercase().contains("authorization:"));
                first
                    .write_all(
                        b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"lab\", qop=\"auth\", nonce=\"n12345\", opaque=\"op1\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();

                let (mut second, _) = listener.accept().await.unwrap();
                let req2 = read_h1_request_head(&mut second).await.unwrap();
                assert!(
                    req2.to_ascii_lowercase().contains("authorization: digest "),
                    "expected Digest challenge retry, got: {req2}"
                );
                second
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\ndigest-ok")
                    .await
                    .unwrap();
            });
            let mut config = SendConfig::default();
            config.auth = crate::AuthConfig {
                platform_enabled: true,
                platform_rules: std::sync::Arc::from([crate::PlatformAuthRuleConfig {
                    enabled: true,
                    destination_host: "*".into(),
                    credentials: crate::AuthCredentials {
                        scheme: crate::HttpAuthScheme::Digest,
                        username: "bob".into(),
                        password: "pw".into(),
                        ..Default::default()
                    },
                }]),
                upstream_rules: std::sync::Arc::from([]),
            };
            let req = HttpRequest {
                method: "GET".into(),
                url: format!("http://{address}/digest"),
                version: HttpVersion::Http11,
                headers: Vec::new(),
                body: Vec::new(),
                trailers: Vec::new(),
            };
            let metrics = prepare_send(&req, config)
                .unwrap()
                .send_with_metrics(CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(metrics.response.status, 200);
            assert_eq!(metrics.response.body, b"digest-ok");
            origin.await.unwrap();
        }

        // 3. Upstream HTTP proxy Basic auth via PreparedSend::send.
        {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy_addr = listener.local_addr().unwrap();
            let proxy_task = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let head = read_h1_request_head(&mut stream).await.unwrap();
                assert!(
                    head.to_ascii_lowercase().contains("proxy-authorization: basic "),
                    "expected Proxy-Authorization header, got: {head}"
                );
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nproxy-ok")
                    .await
                    .unwrap();
            });
            let mut config = SendConfig::default();
            config.network.upstream_route = crate::UpstreamRoute::Http(crate::ProxyEndpoint {
                host: "127.0.0.1".into(),
                port: proxy_addr.port(),
                credentials: None,
            });
            config.auth.upstream_rules = std::sync::Arc::from([crate::UpstreamAuthRuleConfig {
                enabled: true,
                destination_host: "*".into(),
                proxy_host: "*".into(),
                proxy_port: 0,
                credentials: crate::AuthCredentials {
                    scheme: crate::HttpAuthScheme::Basic,
                    username: "proxy-u".into(),
                    password: "proxy-p".into(),
                    ..Default::default()
                },
            }]);
            let req = HttpRequest {
                method: "GET".into(),
                url: "http://upstream.lab.invalid/proxied".into(),
                version: HttpVersion::Http11,
                headers: Vec::new(),
                body: Vec::new(),
                trailers: Vec::new(),
            };
            let resp = prepare_send(&req, config)
                .unwrap()
                .send(CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(resp.status, 200);
            assert_eq!(resp.body, b"proxy-ok");
            proxy_task.await.unwrap();
        }
    })
    .await
    .expect("PreparedSend auth flows complete promptly");
}
