use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use super::*;
use crate::*;

mod cookie_jar_tests;
mod pipeline_tests;
mod rules_and_macros_tests;

pub(super) async fn read_http_request(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let header_end = loop {
        let n = stream.read(&mut chunk).await.expect("read request head");
        if n == 0 {
            break None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break Some(pos + 4);
        }
    };
    if let Some(end) = header_end {
        let head_str = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
        let expected_body = head_str
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        while buf.len() < end + expected_body {
            let n = stream.read(&mut chunk).await.expect("read request body");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

pub(super) async fn write_http_response(
    stream: &mut TcpStream,
    status_line: &str,
    extra_headers: &[(&str, &str)],
    body: &str,
) {
    let mut raw = format!(
        "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in extra_headers {
        raw.push_str(&format!("{k}: {v}\r\n"));
    }
    raw.push_str("\r\n");
    raw.push_str(body);
    stream
        .write_all(raw.as_bytes())
        .await
        .expect("write response");
    let _ = stream.shutdown().await;
}

pub(super) fn sample_request(url: &str) -> HttpRequest {
    HttpRequest {
        method: "GET".into(),
        url: url.into(),
        version: HttpVersion::Http11,
        headers: vec![Header {
            name: "Host".into(),
            value: b"app.example.com".to_vec(),
        }],
        body: Vec::new(),
        trailers: Vec::new(),
    }
}

pub(super) async fn next_matching<F>(client: &mut Client, mut pred: F) -> Event
where
    F: FnMut(&Event) -> bool,
{
    while let Some(ev) = client.events.recv().await {
        if pred(&ev) {
            return ev;
        }
    }
    panic!("event stream closed before matching event arrived");
}

pub(super) async fn recv_matching<F>(
    client: &mut Client,
    latest_ws: &mut Workspace,
    mut pred: F,
) -> Event
where
    F: FnMut(&Event) -> bool,
{
    while let Some(ev) = client.events.recv().await {
        if let Event::Error {
            operation, message, ..
        } = &ev
        {
            panic!("unexpected Event::Error({operation}): {message}");
        }
        if let Event::WorkspaceChanged { workspace, .. } = &ev {
            *latest_ws = workspace.clone();
        }
        if pred(&ev) {
            return ev;
        }
    }
    panic!("event stream closed before matching event arrived");
}

pub(super) async fn save_project_to_disk(
    client: &mut Client,
    project: ProjectKey,
    path: &std::path::Path,
    latest_ws: &mut Workspace,
) {
    client
        .commands
        .send(Command::SaveProject {
            project,
            path: Some(path.to_path_buf()),
            workspace: None,
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
        })
        .await
        .unwrap();
    recv_matching(
        client,
        latest_ws,
        |e| matches!(e, Event::ProjectSaved { path: saved, .. } if saved == path),
    )
    .await;
}

pub(super) async fn spawn_auth_and_target_servers() -> (
    std::net::SocketAddr,
    Arc<std::sync::atomic::AtomicBool>,
    tokio::task::JoinHandle<()>,
    std::net::SocketAddr,
    Arc<AtomicUsize>,
    Arc<std::sync::Mutex<Vec<String>>>,
    tokio::task::JoinHandle<()>,
) {
    let auth_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let auth_addr = auth_listener.local_addr().unwrap();
    let fail_macro = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fail_macro_srv = fail_macro.clone();

    let auth_srv = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = auth_listener.accept().await else {
                break;
            };
            let should_fail = fail_macro_srv.load(Ordering::SeqCst);
            tokio::spawn(async move {
                let _ = read_http_request(&mut stream).await;
                if should_fail {
                    write_http_response(&mut stream, "500 Internal Error", &[], "macro_down").await;
                } else {
                    write_http_response(
                        &mut stream,
                        "200 OK",
                        &[("Set-Cookie", "dyn_sid=macro_cookie_1; Path=/")],
                        "csrf_token=tok_pipeline_7",
                    )
                    .await;
                }
            });
        }
    });

    let target_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_addr = target_listener.local_addr().unwrap();
    let target_hits = Arc::new(AtomicUsize::new(0));
    let target_hits_srv = target_hits.clone();
    let seen_requests = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen_requests_srv = seen_requests.clone();

    let target_srv = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = target_listener.accept().await else {
                break;
            };
            let hits = target_hits_srv.clone();
            let seen = seen_requests_srv.clone();
            tokio::spawn(async move {
                let raw = read_http_request(&mut stream).await;
                hits.fetch_add(1, Ordering::SeqCst);
                seen.lock().unwrap().push(raw);
                write_http_response(&mut stream, "200 OK", &[], "target_ok").await;
            });
        }
    });

    (
        auth_addr,
        fail_macro,
        auth_srv,
        target_addr,
        target_hits,
        seen_requests,
        target_srv,
    )
}
