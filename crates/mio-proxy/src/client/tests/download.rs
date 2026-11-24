use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::*;
use crate::{DownloadConfig, DownloadRouteResolver, DownloadSink, download_to_sink, download_to_writer};

#[tokio::test]
async fn download_to_writer_streams_chunks_and_follows_redirects() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for hop in 0..2 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 2048];
                let n = sock.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]);
                if hop == 0 {
                    assert!(req.starts_with("GET /start HTTP/1.1\r\n"));
                    let resp = format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{addr}/payload\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    sock.write_all(resp.as_bytes()).await.unwrap();
                } else {
                    assert!(req.starts_with("GET /payload HTTP/1.1\r\n"));
                    let head = "HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n";
                    sock.write_all(head.as_bytes()).await.unwrap();
                    sock.write_all(b"hello ").await.unwrap();
                    sock.flush().await.unwrap();
                    tokio::time::sleep(Duration::from_millis(15)).await;
                    sock.write_all(b"world").await.unwrap();
                    sock.flush().await.unwrap();
                }
            }
        });

        let mut out = Vec::new();
        let mut progress = Vec::new();
        let mut cfg = DownloadConfig::default();
        cfg.headers.push(Header {
            name: "Accept".into(),
            value: b"*/*".to_vec(),
        });
        let outcome = download_to_writer(
            &format!("http://{addr}/start"),
            cfg,
            &mut out,
            CancellationToken::new(),
            |received, total| progress.push((received, total)),
        )
        .await
        .unwrap();

        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.final_url, format!("http://{addr}/payload"));
        assert_eq!(outcome.bytes_written, 11);
        assert_eq!(outcome.content_length, Some(11));
        assert_eq!(&out, b"hello world");
        assert_eq!(progress.last().copied(), Some((11, Some(11))));
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn download_to_sink_enforces_body_limits_redirects_and_non_2xx_snippets() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");
                    match path {
                        "/loop" => {
                            let resp = format!(
                                "HTTP/1.1 302 Found\r\nLocation: http://{addr}/loop\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            );
                            let _ = sock.write_all(resp.as_bytes()).await;
                        }
                        "/too-big-cl" => {
                            let resp = "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n";
                            let _ = sock.write_all(resp.as_bytes()).await;
                        }
                        "/too-big-chunked" => {
                            let resp = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\n12345\r\n5\r\n67890\r\n0\r\n\r\n";
                            let _ = sock.write_all(resp.as_bytes()).await;
                        }
                        "/not-found" => {
                            let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 11\r\nConnection: close\r\n\r\nmissing-zip";
                            let _ = sock.write_all(resp.as_bytes()).await;
                        }
                        _ => {}
                    }
                });
            }
        });

        struct CountingSink {
            calls: Arc<AtomicUsize>,
            bytes: Vec<u8>,
        }
        impl DownloadSink for CountingSink {
            async fn write_chunk(
                &mut self,
                chunk: &[u8],
                _total_received: u64,
                _content_length: Option<u64>,
            ) -> Result<(), ProxyError> {
                self.calls.fetch_add(1, Ordering::Relaxed);
                self.bytes.extend_from_slice(chunk);
                Ok(())
            }
        }
        struct DirectResolver;
        impl DownloadRouteResolver for DirectResolver {
            async fn resolve_route(&mut self, _target_url: &str) -> UpstreamRoute {
                UpstreamRoute::Direct
            }
        }

        // Content-Length limit checked before invoking sink
        let calls = Arc::new(AtomicUsize::new(0));
        let mut sink = CountingSink {
            calls: calls.clone(),
            bytes: Vec::new(),
        };
        let cfg = DownloadConfig {
            max_body_bytes: 8,
            ..Default::default()
        };
        let err = download_to_sink(
            &format!("http://{addr}/too-big-cl"),
            cfg.clone(),
            DirectResolver,
            &mut sink,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ProxyError::BodyLimit { limit: 8 }));
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        // Chunked mid-stream limit
        let err = download_to_sink(
            &format!("http://{addr}/too-big-chunked"),
            cfg.clone(),
            DirectResolver,
            &mut sink,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ProxyError::BodyLimit { limit: 8 }));

        // Non-2xx captures error_snippet without writing to sink
        let mut sink_404 = CountingSink {
            calls: Arc::new(AtomicUsize::new(0)),
            bytes: Vec::new(),
        };
        let outcome = download_to_sink(
            &format!("http://{addr}/not-found"),
            DownloadConfig::default(),
            DirectResolver,
            &mut sink_404,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.status, 404);
        assert_eq!(outcome.bytes_written, 0);
        assert_eq!(&outcome.error_snippet, b"missing-zip");
        assert!(sink_404.bytes.is_empty());

        // Max redirects enforced
        let cfg_redir = DownloadConfig {
            max_redirects: 2,
            ..Default::default()
        };
        let err = download_to_sink(
            &format!("http://{addr}/loop"),
            cfg_redir,
            DirectResolver,
            &mut sink_404,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ProxyError::Network(_)));

        // on_response_head receives status, headers, and content-length before chunks
        let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr2 = listener2.local_addr().unwrap();
        let srv2 = tokio::spawn(async move {
            let (mut sock, _) = listener2.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await.unwrap();
            let resp = "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 5-9/10\r\nContent-Length: 5\r\nConnection: close\r\n\r\n56789";
            sock.write_all(resp.as_bytes()).await.unwrap();
        });
        struct HeadCapturingSink {
            seen_head: Option<(u16, Option<u64>, Option<String>)>,
            bytes: Vec<u8>,
        }
        impl DownloadSink for HeadCapturingSink {
            async fn on_response_head(
                &mut self,
                status: u16,
                headers: &[Header],
                content_length: Option<u64>,
            ) -> Result<(), ProxyError> {
                let cr = headers
                    .iter()
                    .find(|h| h.name.eq_ignore_ascii_case("content-range"))
                    .and_then(|h| std::str::from_utf8(&h.value).ok())
                    .map(str::to_owned);
                self.seen_head = Some((status, content_length, cr));
                Ok(())
            }

            async fn write_chunk(
                &mut self,
                chunk: &[u8],
                _total_received: u64,
                _content_length: Option<u64>,
            ) -> Result<(), ProxyError> {
                self.bytes.extend_from_slice(chunk);
                Ok(())
            }
        }
        let mut head_sink = HeadCapturingSink {
            seen_head: None,
            bytes: Vec::new(),
        };
        let head_out = download_to_sink(
            &format!("http://{addr2}/part"),
            DownloadConfig::default(),
            DirectResolver,
            &mut head_sink,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(head_out.status, 206);
        assert_eq!(
            head_sink.seen_head,
            Some((206, Some(5), Some("bytes 5-9/10".into())))
        );
        assert_eq!(&head_sink.bytes, b"56789");
        srv2.await.unwrap();

        server.abort();
    })
    .await
    .unwrap();
}
