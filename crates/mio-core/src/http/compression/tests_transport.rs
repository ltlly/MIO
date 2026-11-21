use std::{
    cell::Cell,
    time::{Duration, Instant},
};

use brotli_decompressor::{BrotliDecompressStream, BrotliResult, BrotliState, StandardAlloc};
use mio_plugin_api::HttpVersion;
use tokio_util::sync::CancellationToken;

use super::{tests::*, *};
use crate::http::{parse_response, response_to_raw};

#[tokio::test]
async fn negotiated_http2_content_decoding_preserves_trailers_without_transfer_encoding() {
    use anyhow::{Context, ensure};
    use mio_plugin_api::{HttpRequest, ProjectKey, Uuid};
    use mio_proxy::{
        InterceptDecision, MitmAuthority, ProxyConfig, ProxyEndpoint, ProxyEvent, SendConfig,
        UpstreamRoute, UpstreamTls,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::mpsc,
        time::{Instant as TokioInstant, timeout_at},
    };

    let deadline = TokioInstant::now() + Duration::from_secs(8);
    let ca = tokio::task::spawn_blocking(MitmAuthority::generate)
        .await
        .unwrap()
        .unwrap();
    let certificate = ca.certificate_der();
    let mut config = ProxyConfig::new(ca);
    config.hooks.intercept_requests = true;
    config.limits.request_timeout = Duration::from_secs(2);
    config.limits.intercept_timeout = Duration::from_secs(2);
    config.limits.max_body_bytes = 256;
    let (events, mut receiver) = mpsc::channel(16);
    let mut proxy = mio_proxy::start(
        config,
        ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
        events,
    )
    .await
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let encoded = gzip(BINARY);
    let body = encoded.clone();
    let origin = tokio::spawn(async move {
        timeout_at(deadline, async {
            let (mut stream, peer) = listener.accept().await?;
            ensure!(peer.ip().is_loopback(), "non-loopback fixture peer");
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                ensure!(head.len() < 4096, "fixture head exceeded its bound");
                head.push(stream.read_u8().await?);
            }
            ensure!(head.starts_with(b"POST /compression-h2 HTTP/1.1\r\n"));
            let mut request_body = [0; BINARY.len()];
            stream.read_exact(&mut request_body).await?;
            ensure!(request_body == BINARY);
            let mut wire = format!(
                "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nTransfer-Encoding: chunked\r\nTrailer: X-End\r\n\r\n{:x}\r\n",
                body.len()
            )
            .into_bytes();
            wire.extend_from_slice(&body);
            wire.extend_from_slice(b"\r\n0\r\nX-End: one\r\nX-End: two\r\n\r\n");
            stream.write_all(&wire).await?;
            ensure!(stream.read(&mut [0]).await? == 0, "POST was retried");
            Ok::<_, anyhow::Error>(listener)
        })
        .await
        .context("HTTP/2 fixture origin deadline")?
    });
    let cancel = CancellationToken::new();
    let mut sending = tokio::spawn(mio_proxy::send(
        HttpRequest {
            method: "POST".into(),
            url: format!("https://localhost:{}/compression-h2", address.port()),
            version: HttpVersion::Http2,
            headers: vec![],
            body: BINARY.to_vec(),
            trailers: vec![],
        },
        SendConfig {
            network: mio_proxy::SendNetworkConfig {
                upstream_route: UpstreamRoute::Http(ProxyEndpoint {
                    host: "127.0.0.1".into(),
                    port: proxy.address().port(),
                    credentials: None,
                }),
                ..Default::default()
            },
            tls: mio_proxy::UpstreamTlsConfig {
                trust: UpstreamTls::CustomRoots {
                    certificates_der: vec![certificate],
                    include_system: false,
                },
                ..Default::default()
            },
            limits: mio_proxy::SendLimits {
                timeout: Duration::from_secs(3),
                ..Default::default()
            },
            http: mio_proxy::SendHttpConfig {
                version: Some(HttpVersion::Http2),
                ..Default::default()
            },
            ..SendConfig::default()
        },
        cancel.clone(),
    ));
    let intercepted = timeout_at(deadline, async {
        loop {
            let event = receiver
                .recv()
                .await
                .context("fixture proxy events closed")?;
            if let ProxyEvent::Intercepted {
                mut record, reply, ..
            } = event
            {
                ensure!(record.request.version == HttpVersion::Http2);
                ensure!(record.request.body == BINARY);
                // Terminate only the fixture's H2 TLS leg; its origin remains local plaintext H1.
                record.request.url = format!("http://{address}/compression-h2");
                record.request.version = HttpVersion::Http11;
                reply
                    .send(InterceptDecision::ForwardRequest(record.request))
                    .map_err(|_| anyhow::anyhow!("fixture interception closed"))?;
                return Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await;
    if !matches!(intercepted, Ok(Ok(()))) {
        cancel.cancel();
    }
    let sent = match timeout_at(deadline, &mut sending).await {
        Ok(result) => result,
        Err(_) => {
            cancel.cancel();
            sending.await
        }
    };
    proxy.request_stop();
    let stopped = proxy.join().await;
    let origin_result = origin.await;
    assert!(
        TokioInstant::now() <= deadline,
        "fixture exceeded eight seconds"
    );
    intercepted.unwrap().unwrap();
    stopped.unwrap();
    let listener = origin_result.unwrap().unwrap().into_std().unwrap();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let received = sent.unwrap().unwrap();
    assert_eq!(received.version, HttpVersion::Http2);
    assert_eq!(received.body, encoded);
    assert!(
        !received
            .headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case("transfer-encoding"))
    );
    assert_eq!(received.trailers.len(), 2);
    assert!(
        received
            .trailers
            .iter()
            .all(|header| header.name.eq_ignore_ascii_case("x-end"))
    );
    assert_eq!(received.trailers[0].value, b"one");
    assert_eq!(received.trailers[1].value, b"two");
    let before = received.clone();
    let decoded = unpack(&received, 256).unwrap().unwrap();
    assert_eq!(received, before);
    assert_eq!(decoded.version, HttpVersion::Http2);
    assert_eq!(decoded.body, BINARY);
    assert_eq!(decoded.trailers, received.trailers);
    assert!(!decoded.headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("content-encoding")
            || header.name.eq_ignore_ascii_case("content-length")
            || header.name.eq_ignore_ascii_case("transfer-encoding")
    }));
}

#[test]
fn chunked_and_trailers_preserve_fields_and_raw_framing() {
    for (with_chunked, with_trailers) in [(true, false), (false, true), (true, true)] {
        let mut original = response(b"gzip", gzip(BINARY));
        if with_chunked {
            original
                .headers
                .push(field("Transfer-Encoding", b"chunked"));
        }
        if with_trailers {
            original.trailers = vec![field("X-End", b"one"), field("X-End", b"\xfftwo")];
        }
        let decoded = unpack(&original, 256).unwrap().unwrap();
        assert!(
            !decoded
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("content-length")
                    || h.name.eq_ignore_ascii_case("content-encoding"))
        );
        assert_eq!(decoded.trailers, original.trailers);
        let raw = parse_response(&response_to_raw(&decoded), decoded.version).unwrap();
        assert_eq!(raw.body, BINARY);
        assert_eq!(raw.trailers, original.trailers);
        assert!(is_chunked(&raw.headers));
    }
    let mut original = response(b"gzip", gzip(BINARY));
    original
        .headers
        .retain(|h| !h.name.eq_ignore_ascii_case("content-length"));
    let decoded = unpack(&original, 256).unwrap().unwrap();
    assert_eq!(
        decoded.headers.last().unwrap(),
        &field("Content-Length", BINARY.len().to_string().as_bytes())
    );
}

#[test]
fn strict_brotli_rejects_large_window_extension() {
    let input = [0x11, 0xd8];
    let mut state = BrotliState::new(
        StandardAlloc::default(),
        StandardAlloc::default(),
        StandardAlloc::default(),
    );
    let mut available_in = input.len();
    let mut input_offset = 0;
    let mut output = [0; 1];
    let mut available_out = output.len();
    let mut output_offset = 0;
    let mut total_out = 0;
    assert!(matches!(
        BrotliDecompressStream(
            &mut available_in,
            &mut input_offset,
            &input,
            &mut available_out,
            &mut output_offset,
            &mut output,
            &mut total_out,
            &mut state
        ),
        BrotliResult::ResultSuccess
    ));
    assert_eq!((input_offset, output_offset), (input.len(), 0));
    failed(&response(b"br", input.to_vec()), 32);
}

#[test]
fn cancellation_and_deadline_are_distinct() {
    for (encoding, body) in [
        (b"gzip".as_slice(), gzip(BINARY)),
        (b"deflate", deflate(BINARY)),
        (b"br", br_stored(BINARY)),
    ] {
        let original = response(encoding, body);
        let cancel = CancellationToken::new();
        assert_eq!(
            unpack_response(&original, "GET", 256, &cancel, Instant::now()).unwrap_err(),
            UnpackError::Deadline
        );
        cancel.cancel();
        assert_eq!(
            unpack_response(
                &original,
                "GET",
                256,
                &cancel,
                Instant::now() + Duration::from_secs(5)
            )
            .unwrap_err(),
            UnpackError::Cancelled
        );
    }
}

#[test]
fn codecs_poll_control_before_finishing() {
    let mut seed = 1u32;
    let plain = (0..CHUNK * 2 + 37)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 24) as u8
        })
        .collect::<Vec<_>>();
    for (coding, encoded) in [
        (Coding::Gzip, gzip(&plain)),
        (Coding::Deflate, deflate(&plain)),
        (Coding::Brotli, br_stored(&plain)),
    ] {
        assert!(encoded.len() > CHUNK);
        let calls = Cell::new(0);
        let count = || {
            calls.set(calls.get() + 1);
            Ok(())
        };
        assert_eq!(
            decode_layer(coding, &encoded, CHUNK * 3, &count).unwrap(),
            plain
        );
        assert!(calls.get() > 3);
        let stop_at = calls.get() - 1;
        for by_cancel in [true, false] {
            let cancel = CancellationToken::new();
            let deadline = Cell::new(Instant::now() + Duration::from_secs(5));
            calls.set(0);
            let check = || {
                calls.set(calls.get() + 1);
                if calls.get() == stop_at {
                    if by_cancel {
                        cancel.cancel();
                    } else {
                        deadline.set(Instant::now());
                    }
                }
                check_control(&cancel, deadline.get())
            };
            let error = decode_layer(coding, &encoded, CHUNK * 3, &check).unwrap_err();
            assert_eq!(
                error,
                if by_cancel {
                    UnpackError::Cancelled
                } else {
                    UnpackError::Deadline
                }
            );
            assert_eq!(calls.get(), stop_at);
        }
    }
}
