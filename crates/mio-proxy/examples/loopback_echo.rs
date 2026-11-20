//! Local, non-sensitive GUI fixture. POST a body with duplicate X-Test headers;
//! the response echoes both. GET /__shutdown closes the fixture after its response.

use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, Response, StatusCode, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use tokio::{net::TcpListener, task::JoinSet};
use tokio_util::sync::CancellationToken;

const MAX_BODY: usize = 64 * 1024;

async fn echo(
    request: Request<Incoming>,
    shutdown_after_response: Arc<AtomicBool>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    if request.uri().path() == "/__shutdown" {
        shutdown_after_response.store(true, Ordering::SeqCst);
        return Ok(Response::new(Full::new(Bytes::from_static(b"stopping\n"))));
    }
    let method = request.method().clone();
    let headers = request
        .headers()
        .get_all("x-test")
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let body = match Limited::new(request.into_body(), MAX_BODY).collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => {
            let mut response = Response::new(Full::new(Bytes::from_static(
                b"invalid body or body exceeds 64 KiB\n",
            )));
            *response.status_mut() = StatusCode::PAYLOAD_TOO_LARGE;
            return Ok(response);
        }
    };
    println!(
        "RECEIVED method={method} bytes={} x_test_count={} matches_chinese_fixture={}",
        body.len(),
        headers.len(),
        body.as_ref() == "最新中文🙂".as_bytes()
    );
    let length = body.len();
    let mut response = Response::new(Full::new(body));
    response
        .headers_mut()
        .insert("content-type", "application/octet-stream".parse().unwrap());
    response
        .headers_mut()
        .insert("x-mio-method", method.as_str().parse().unwrap());
    response
        .headers_mut()
        .insert("x-mio-body-bytes", length.into());
    response
        .headers_mut()
        .insert("x-mio-x-test-count", headers.len().into());
    for value in headers {
        response.headers_mut().append("x-observed-test", value);
    }
    Ok(response)
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    println!("READY http://{}", listener.local_addr()?);
    println!(
        "POST /echo with X-Test: one, X-Test: two and body 最新中文🙂; GET /__shutdown to stop"
    );
    let cancel = CancellationToken::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15 * 60);
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep_until(deadline) => break,
            Some(result) = tasks.join_next(), if !tasks.is_empty() => { result?; },
            accepted = listener.accept(), if tasks.len() < 16 => {
                let (stream, _) = accepted?;
                let cancel = cancel.clone();
                tasks.spawn(async move {
                    let shutdown = Arc::new(AtomicBool::new(false));
                    let request_shutdown = shutdown.clone();
                    let service = service_fn(move |request| echo(request, request_shutdown.clone()));
                    let mut builder = hyper::server::conn::http1::Builder::new();
                    builder.keep_alive(false);
                    tokio::select! {
                        _ = cancel.cancelled() => {},
                        _ = tokio::time::timeout(Duration::from_secs(30), builder.serve_connection(TokioIo::new(stream), service)) => {},
                    }
                    if shutdown.load(Ordering::SeqCst) { cancel.cancel(); }
                });
            }
        }
    }
    drop(listener);
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(result) = tasks.join_next().await {
            result?;
        }
        Ok::<_, tokio::task::JoinError>(())
    })
    .await??;
    println!("STOPPED: listener and accepted connections closed");
    Ok(())
}
