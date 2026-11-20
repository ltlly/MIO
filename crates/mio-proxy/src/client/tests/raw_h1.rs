use super::*;

#[tokio::test]
async fn disabled_update_content_length_preserves_manual_and_omitted_headers_on_wire() {
    tokio::time::timeout(Duration::from_secs(5), async {
        for version in [HttpVersion::Http11, HttpVersion::Http10] {
            for method in ["POST", "GET"] {
                for lengths in [vec!["99"], vec!["2"], vec!["0"], vec![], vec!["2", "4"]] {
                    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let address = listener.local_addr().unwrap();
                    let headers: Vec<Header> = lengths
                        .iter()
                        .map(|length| Header {
                            name: "Content-Length".into(),
                            value: length.as_bytes().to_vec(),
                        })
                        .collect();
                    let mut expected = format!(
                        "{method} /manual-cl HTTP/1.{}\r\n",
                        if version == HttpVersion::Http10 { 0 } else { 1 }
                    )
                    .into_bytes();
                    for length in lengths {
                        expected.extend_from_slice(format!("Content-Length: {length}\r\n").as_bytes());
                    }
                    expected.extend_from_slice(format!("host: {address}\r\n\r\nabcd").as_bytes());
                    let origin = tokio::spawn(async move {
                        let (mut stream, _) = listener.accept().await.unwrap();
                        let mut received = vec![0; expected.len()];
                        stream.read_exact(&mut received).await.unwrap();
                        assert_eq!(received, expected, "headers and all body bytes must arrive unchanged");
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                            .await
                            .unwrap();
                        assert_eq!(stream.read(&mut [0]).await.unwrap(), 0, "no synthesized chunk terminator or duplicate body may follow");
                        listener
                    });
                    let request = HttpRequest {
                        method: method.into(),
                        url: format!("http://{address}/manual-cl"),
                        version,
                        headers,
                        body: b"abcd".to_vec(),
                        trailers: Vec::new(),
                    };
                    let config = SendConfig {
                        http_version: Some(version),
                        update_content_length: false,
                        ..SendConfig::default()
                    };
                    let response = if version == HttpVersion::Http11 {
                        send(request, config, CancellationToken::new()).await.unwrap()
                    } else {
                        prepare_send(&request, config)
                            .unwrap()
                            .send_with_metrics(CancellationToken::new())
                            .await
                            .unwrap()
                            .response
                    };
                    assert_eq!(response.status, 200);
                    assert_eq!(response.body, b"ok");
                    assert_eq!(origin.await.unwrap().into_std().unwrap().accept().unwrap_err().kind(), ErrorKind::WouldBlock);
                }
            }
        }
    })
    .await
    .expect("raw wire Content-Length checks complete within bounded fixture");
}

#[tokio::test]
async fn http1_cookie_fields_keep_independent_values_and_binary_bytes_on_the_actual_socket() {
    tokio::time::timeout(Duration::from_secs(4), async {
        for prepared in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let cookies = [
                b"first=1; token=\"two;three\"".to_vec(),
                b"token=second; raw=\xff; percent=%41".to_vec(),
            ];
            let origin = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    assert!(head.len() < 8192);
                    head.push(stream.read_u8().await.unwrap());
                }
                let mut body = [0; 3];
                stream.read_exact(&mut body).await.unwrap();
                stream
                    .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 3\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
                stream.write_all(RESPONSE).await.unwrap();
                let eof = stream.read(&mut [0]).await.unwrap();
                (listener.into_std().unwrap(), head, body, eof)
            });
            let request = HttpRequest {
                method: "POST".into(),
                url: format!("http://{address}/cookie-fields"),
                version: HttpVersion::Http11,
                headers: vec![
                    Header { name: "Cookie".into(), value: cookies[0].clone() },
                    Header { name: "X-Duplicate".into(), value: b"one".to_vec() },
                    Header { name: "Cookie".into(), value: cookies[1].clone() },
                    Header { name: "X-Duplicate".into(), value: b"two".to_vec() },
                    Header { name: "X-Keep".into(), value: b"preserved".to_vec() },
                    Header { name: "Connection".into(), value: b"close, X-Secret".to_vec() },
                    Header { name: "X-Secret".into(), value: b"must-not-reach-origin".to_vec() },
                    Header { name: "Proxy-Authorization".into(), value: b"must-not-reach-origin".to_vec() },
                ],
                body: PAYLOAD.to_vec(),
                trailers: Vec::new(),
            };
            let original = request.clone();
            let cancel = CancellationToken::new();
            let response = if prepared {
                prepare_send(&request, SendConfig::default())
                    .unwrap()
                    .send_with_metrics(cancel.clone())
                    .await
                    .unwrap()
                    .response
            } else {
                send(request.clone(), SendConfig::default(), cancel.clone())
                    .await
                    .unwrap()
            };
            let (listener, head, body, eof) = origin.await.unwrap();
            assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
            assert_eq!(eof, 0);
            assert_eq!(body, PAYLOAD);
            assert_eq!(response.status, 201);
            assert_eq!(response.body, RESPONSE);
            assert_eq!(request, original);
            assert!(!cancel.is_cancelled());
            assert!(head.starts_with(b"POST /cookie-fields HTTP/1.1\r\n"));
            let fields = head
                .split(|byte| *byte == b'\n')
                .filter_map(|line| {
                    let line = line.strip_suffix(b"\r").unwrap_or(line);
                    let colon = line.iter().position(|byte| *byte == b':')?;
                    let value = line[colon + 1..].strip_prefix(b" ").unwrap_or(&line[colon + 1..]);
                    Some((&line[..colon], value))
                })
                .collect::<Vec<_>>();
            let actual_cookies = fields
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case(b"cookie"))
                .map(|(_, value)| *value)
                .collect::<Vec<_>>();
            assert_eq!(actual_cookies, cookies.iter().map(Vec::as_slice).collect::<Vec<_>>());
            assert_eq!(fields.iter().filter(|(name, _)| name.eq_ignore_ascii_case(b"x-duplicate")).map(|(_, value)| *value).collect::<Vec<_>>(), vec![b"one".as_slice(), b"two".as_slice()]);
            assert!(fields.iter().any(|(name, value)| name.eq_ignore_ascii_case(b"x-keep") && *value == b"preserved"));
            assert!(fields.iter().any(|(name, value)| name.eq_ignore_ascii_case(b"content-length") && *value == b"3"));
            assert!(fields.iter().all(|(name, _)| ![b"x-secret".as_slice(), b"proxy-authorization", b"connection", b"transfer-encoding"].iter().any(|excluded| name.eq_ignore_ascii_case(excluded))));
        }
    })
    .await
    .expect("two Cookie fields, original binary values and both send owners finish and join within budget");
}

#[tokio::test]
async fn http1_cookie_override_handles_fragmented_headers_without_reframing_body() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let cookies: Arc<[u8]> = Arc::from(b"Cookie: first=1\r\nCookie: raw=\xff\r\n".as_slice());
        for h2_preface in [false, true] {
            let input = if h2_preface {
                b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n\0\xffbinary-frame".to_vec()
            } else {
                b"POST /fragmented HTTP/1.1\r\nCookie: first=1; raw=\xff\r\nX-Duplicate: one\r\nX-Duplicate: two\r\nTransfer-Encoding: chunked\r\nTrailer: X-End\r\n\r\n3\r\n\0\xffP\r\n0\r\nX-End: last\r\n\r\n".to_vec()
            };
            let expected = if h2_preface {
                input.clone()
            } else {
                b"POST /fragmented HTTP/1.1\r\nCookie: first=1\r\nCookie: raw=\xff\r\nX-Duplicate: one\r\nX-Duplicate: two\r\nTransfer-Encoding: chunked\r\nTrailer: X-End\r\n\r\n3\r\n\0\xffP\r\n0\r\nX-End: last\r\n\r\n".to_vec()
            };
            let (writer, mut reader) = tokio::io::duplex(1);
            let source = tokio::spawn(async move {
                let mut received = Vec::new();
                reader.read_to_end(&mut received).await.unwrap();
                received
            });
            let mut writer = RawH1Io {
                inner: writer,
                pending: None,
                cookie_fields: Some((cookies.clone(), 1024)),
                cookie_head: Vec::new(),
                cookie_offset: 0,
            };
            for fragment in input.chunks(3) {
                if h2_preface {
                    let split = fragment.len().min(1);
                    let bufs = [IoSlice::new(&fragment[..split]), IoSlice::new(&fragment[split..])];
                    let written = writer.write_vectored(&bufs).await.unwrap();
                    assert!(written > 0 && written <= fragment.len());
                    writer.write_all(&fragment[written..]).await.unwrap();
                } else {
                    writer.write_all(fragment).await.unwrap();
                }
            }
            writer.flush().await.unwrap();
            writer.shutdown().await.unwrap();
            drop(writer);
            assert_eq!(source.await.unwrap(), expected);
        }
        let (writer, mut reader) = tokio::io::duplex(1);
        let mut writer = RawH1Io {
            inner: writer,
            pending: None,
            cookie_fields: Some((cookies, 1024)),
            cookie_head: Vec::new(),
            cookie_offset: 0,
        };
        writer.write_all(b"POST /partial HTTP/1.1\r\nCookie:").await.unwrap();
        assert_eq!(writer.flush().await.unwrap_err().kind(), ErrorKind::InvalidData);
        drop(writer);
        let mut received = Vec::new();
        reader.read_to_end(&mut received).await.unwrap();
        assert!(received.is_empty(), "an incomplete head must never expose partial Cookie fields");
    })
    .await
    .expect("fragmented writes, partial flush rejection and untouched H2 bytes remain bounded");
}

#[tokio::test]
async fn raw_http1_override_keeps_lf_on_wire_and_default_normalizes() {
    tokio::time::timeout(Duration::from_secs(4), async {
        for (normalize, update_cl) in [(false, true), (false, false), (true, true)] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let raw: Arc<[u8]> = Arc::from(format!(
                "POST /raw-lf HTTP/1.1\nHost: {address}\nConnection: close, X-Secret\nX-Secret: drop\nProxy-Authorization: Basic dXNlcjpwYXNz\nContent-Length: 99\n\nbody"
            ).into_bytes());
            let expected = format!(
                "POST /raw-lf HTTP/1.1\nHost: {address}\nContent-Length: {}\n\nbody",
                if update_cl { 4 } else { 99 }
            )
            .into_bytes();
            let origin = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                if normalize {
                    let mut head = Vec::new();
                    while !head.ends_with(b"\r\n\r\n") {
                        assert!(head.len() < 1024);
                        head.push(stream.read_u8().await.unwrap());
                    }
                    assert!(head.starts_with(b"POST /raw-lf HTTP/1.1\r\n"));
                    assert!(head.windows(2).filter(|pair| pair[1] == b'\n').all(|pair| pair == b"\r\n"));
                    let mut body = [0; 4];
                    stream.read_exact(&mut body).await.unwrap();
                    assert_eq!(&body, b"body");
                } else {
                    let mut received = vec![0; expected.len()];
                    stream.read_exact(&mut received).await.unwrap();
                    assert_eq!(received, &*expected);
                }
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
                assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
            });
            let request = HttpRequest {
                method: "POST".into(),
                url: format!("http://{address}/raw-lf"),
                version: HttpVersion::Http11,
                headers: vec![
                    Header { name: "Host".into(), value: address.to_string().into_bytes() },
                    Header { name: "Connection".into(), value: b"close, X-Secret".to_vec() },
                    Header { name: "X-Secret".into(), value: b"drop".to_vec() },
                    Header { name: "Proxy-Authorization".into(), value: b"Basic dXNlcjpwYXNz".to_vec() },
                    Header { name: "Content-Length".into(), value: if update_cl { b"4".to_vec() } else { b"99".to_vec() } },
                ],
                body: b"body".to_vec(),
                trailers: Vec::new(),
            };
            let response = prepare_send(&request, SendConfig {
                http_version: Some(HttpVersion::Http11),
                update_content_length: update_cl,
                normalize_http1_line_endings: normalize,
                raw_http1_override: Some(raw),
                ..SendConfig::default()
            }).unwrap().send(CancellationToken::new()).await.unwrap();
            assert_eq!(response.body, b"ok");
            origin.await.unwrap();
        }
    }).await.expect("raw LF and default CRLF sends and cleanup are bounded");
}

#[tokio::test]
async fn raw_http1_override_preserves_chunked_trailers_proxy_route_and_rejects_mismatch() {
    use crate::{ProxyCredentials, ProxyEndpoint};
    tokio::time::timeout(Duration::from_secs(4), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = listener.local_addr().unwrap();
        let raw: Arc<[u8]> = Arc::from(
            b"POST /chunked HTTP/1.1\nHost: example.com:8080\nProxy-Authorization: Basic aG9zdDpsZWFr\nTransfer-Encoding: chunked\n\n2;ext=1\r\nab\r\n2\r\ncd\r\n0\r\nX-End: yes\n\r\n"
                .as_slice(),
        );
        let expected = b"POST http://example.com:8080/chunked HTTP/1.1\nHost: example.com:8080\nTransfer-Encoding: chunked\nproxy-authorization: Basic cHJveHk6c2VjcmV0\n\n2;ext=1\r\nab\r\n2\r\ncd\r\n0\r\nX-End: yes\n\r\n";
        let proxy_peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut received = vec![0; expected.len()];
            stream.read_exact(&mut received).await.unwrap();
            assert_eq!(received, expected);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
            assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
        });
        let request = HttpRequest {
            method: "POST".into(),
            url: "http://example.com:8080/chunked".into(),
            version: HttpVersion::Http11,
            headers: vec![
                Header {
                    name: "Host".into(),
                    value: b"example.com:8080".to_vec(),
                },
                Header {
                    name: "Proxy-Authorization".into(),
                    value: b"Basic aG9zdDpsZWFr".to_vec(),
                },
                Header {
                    name: "Transfer-Encoding".into(),
                    value: b"chunked".to_vec(),
                },
            ],
            body: b"abcd".to_vec(),
            trailers: vec![Header {
                name: "X-End".into(),
                value: b"yes".to_vec(),
            }],
        };
        let config = SendConfig {
            upstream_route: UpstreamRoute::Http(ProxyEndpoint {
                host: proxy_addr.ip().to_string(),
                port: proxy_addr.port(),
                credentials: Some(ProxyCredentials {
                    username: "proxy".into(),
                    password: "secret".into(),
                }),
            }),
            http_version: Some(HttpVersion::Http11),
            normalize_http1_line_endings: false,
            raw_http1_override: Some(raw.clone()),
            ..SendConfig::default()
        };
        for bad_raw in [
            b"POST /escaped HTTP/1.1\nHost: example.com:8080\nProxy-Authorization: Basic aG9zdDpsZWFr\nTransfer-Encoding: chunked\n\n2;ext=1\r\nab\r\n2\r\ncd\r\n0\r\nX-End: yes\n\r\n".as_slice(),
            b"POST /chunked HTTP/1.1\nHost: example.com:8080\nTransfer-Encoding: chunked\n\n2;ext=1\r\nab\r\n2\r\ncd\r\n0\r\nX-End: yes\n\r\n".as_slice(),
            b"POST /chunked HTTP/1.1\nHost: example.com:8080\nProxy-Authorization: Basic aG9zdDpsZWFr\nTransfer-Encoding: chunked\n\n2;ext=1\r\nab\r\n2\r\nxx\r\n0\r\nX-End: yes\n\r\n".as_slice(),
        ] {
            assert!(matches!(
                prepare_send(
                    &request,
                    SendConfig {
                        raw_http1_override: Some(Arc::from(bad_raw)),
                        ..config.clone()
                    }
                ),
                Err(ProxyError::InvalidMessage(_))
            ));
        }
        let response = prepare_send(&request, config)
            .unwrap()
            .send(CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(response.body, b"ok");
        proxy_peer.await.unwrap();
    })
    .await
    .expect("chunked raw override and mismatch checks are bounded");
}
