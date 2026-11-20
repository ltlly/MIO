use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use mio_core::{Client, Command, Event, ProjectKey, Workspace};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    task::JoinSet,
};

use super::{
    metrics::{LatencySummary, ProxyStopReport},
    origin::{fetch_origin_stats, header_val, read_http_head},
};

const CMD_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) async fn send_cmd(client: &Client, command: Command) -> Result<()> {
    client
        .commands
        .send(command)
        .await
        .map_err(|_| anyhow::anyhow!("MIO runtime command channel closed"))
}

pub(crate) async fn recv_matching(
    client: &mut Client,
    accept: impl Fn(&Event) -> bool,
) -> Result<Event> {
    tokio::time::timeout(CMD_TIMEOUT, async {
        loop {
            let event = client
                .events
                .recv()
                .await
                .context("MIO runtime event channel closed")?;
            if let Event::Error {
                operation, message, ..
            } = &event
            {
                bail!("MIO runtime error in {operation}: {message}");
            }
            if matches!(event, Event::Stopped) {
                bail!("MIO runtime stopped unexpectedly");
            }
            if accept(&event) {
                return Ok(event);
            }
        }
    })
    .await
    .context("timed out waiting for MIO runtime event")?
}

pub(crate) async fn wait_project_opened(
    client: &mut Client,
    expected_path: Option<&Path>,
) -> Result<(ProjectKey, Workspace)> {
    let event = recv_matching(client, |ev| matches!(ev, Event::ProjectOpened { .. })).await?;
    let Event::ProjectOpened {
        project,
        path,
        workspace,
    } = event
    else {
        unreachable!()
    };
    ensure!(
        path.as_deref() == expected_path,
        "opened project path {:?} != expected {:?}",
        path,
        expected_path
    );
    Ok((project, workspace))
}

pub(crate) async fn save_project_cmd(
    client: &mut Client,
    project: ProjectKey,
    path: Option<PathBuf>,
) -> Result<f64> {
    let started = Instant::now();
    send_cmd(
        client,
        Command::SaveProject {
            project,
            path,
            workspace: None,
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
        },
    )
    .await?;
    recv_matching(
        client,
        |e| matches!(e, Event::ProjectSaved { project: p, .. } if *p == project),
    )
    .await?;
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

pub(crate) async fn start_proxy_cmd(
    client: &mut Client,
    project: ProjectKey,
) -> Result<SocketAddr> {
    send_cmd(
        client,
        Command::StartProxy {
            project,
            listen: (Ipv4Addr::LOCALHOST, 0).into(),
        },
    )
    .await?;
    let ev = recv_matching(
        client,
        |e| matches!(e, Event::ProxyState { project: p, address: Some(_), .. } if *p == project),
    )
    .await?;
    let Event::ProxyState {
        address: Some(addr),
        ..
    } = ev
    else {
        unreachable!()
    };
    Ok(addr)
}

pub(crate) async fn stop_proxy_cmd(client: &mut Client, project: ProjectKey) -> Result<()> {
    send_cmd(client, Command::StopProxy { project }).await?;
    recv_matching(
        client,
        |e| matches!(e, Event::ProxyState { project: p, address: None, .. } if *p == project),
    )
    .await?;
    Ok(())
}

fn bench_target(index: usize) -> (String, &'static str) {
    match index % 5 {
        0 => (format!("/bench/item-{index}"), "200 "),
        1 => (format!("/bench?id={index}&kind=script"), "200 "),
        2 => (format!("/bench?id={index}&kind=miss"), "404 "),
        _ => (format!("/bench?id={index}&tag=canary"), "200 "),
    }
}

pub(crate) async fn drive_proxy_http_load(
    proxy: SocketAddr,
    origin: SocketAddr,
    count: usize,
    concurrency: usize,
) -> Result<(f64, LatencySummary)> {
    let next = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();
    let mut workers = JoinSet::new();
    for _ in 0..concurrency {
        let next = next.clone();
        workers.spawn(async move {
            let mut latencies = Vec::new();
            let mut conn: Option<TcpStream> = None;
            loop {
                let idx = next.fetch_add(1, Ordering::Relaxed);
                if idx >= count {
                    break;
                }
                let (target, expected_code) = bench_target(idx);
                let raw = format!(
                    "GET http://{origin}{target} HTTP/1.1\r\nHost: {origin}\r\nConnection: keep-alive\r\n\r\n"
                );
                let req_started = Instant::now();
                if conn.is_none() {
                    let stream =
                        tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(proxy))
                            .await??;
                    stream.set_nodelay(true)?;
                    conn = Some(stream);
                }
                let stream = conn.as_mut().expect("connected proxy socket");
                stream.write_all(raw.as_bytes()).await?;
                let head = read_http_head(stream).await?;
                let status_ok = head
                    .lines()
                    .next()
                    .is_some_and(|l| l.starts_with("HTTP/1.1 ") && l[9..].starts_with(expected_code));
                ensure!(status_ok, "unexpected HTTP status line from MIO proxy");
                let len: usize = header_val(&head, "content-length")
                    .context("missing content-length from MIO proxy")?
                    .parse()?;
                ensure!(len <= 256 * 1024, "proxy benchmark body exceeds limit");
                let mut body = vec![0_u8; len];
                if len > 0 {
                    stream.read_exact(&mut body).await?;
                }
                let elapsed_us = req_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
                let text = std::str::from_utf8(&body).context("non-utf8 benchmark body")?;
                ensure!(text.contains(&target), "proxy benchmark payload mismatch");
                if header_val(&head, "connection")
                    .is_some_and(|v| v.eq_ignore_ascii_case("close"))
                {
                    conn = None;
                }
                latencies.push(elapsed_us);
            }
            Ok::<Vec<u64>, anyhow::Error>(latencies)
        });
    }
    let mut samples = Vec::with_capacity(count);
    while let Some(joined) = workers.join_next().await {
        samples.extend(joined??);
    }
    let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    ensure!(samples.len() == count, "proxy load sample count mismatch");
    Ok((wall_ms, LatencySummary::from_samples(&mut samples)))
}

pub(crate) async fn measure_proxy_active_stop(
    client: &mut Client,
    origin: SocketAddr,
    held_conns: usize,
) -> Result<ProxyStopReport> {
    send_cmd(client, Command::NewProject { path: None }).await?;
    let (project, _) = wait_project_opened(client, None).await?;
    let proxy_addr = start_proxy_cmd(client, project).await?;
    let base_stats = fetch_origin_stats(origin).await?;
    let mut holders = JoinSet::new();
    for slot in 0..held_conns {
        holders.spawn(async move {
            let mut stream = TcpStream::connect(proxy_addr).await?;
            stream.set_nodelay(true)?;
            stream
                .write_all(
                    format!(
                        "GET http://{origin}/hold?slot={slot} HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await?;
            let mut buf = [0_u8; 256];
            let _ = stream.read(&mut buf).await;
            Ok::<(), anyhow::Error>(())
        });
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let st = fetch_origin_stats(origin).await?;
            if st.hold_active >= held_conns
                && st.hold_requests.saturating_sub(base_stats.hold_requests) >= held_conns
            {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("origin did not observe held proxy requests before stop")??;
    let stop_started = Instant::now();
    stop_proxy_cmd(client, project).await?;
    while let Some(res) = holders.join_next().await {
        res??;
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if fetch_origin_stats(origin).await?.hold_active == 0 {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    let stop_reclaim_ms = stop_started.elapsed().as_secs_f64() * 1000.0;
    let listener_closed = TcpStream::connect(proxy_addr).await.is_err();
    let after = fetch_origin_stats(origin).await?;
    ensure!(listener_closed, "proxy listener must close after StopProxy");
    ensure!(
        after.hold_closed.saturating_sub(base_stats.hold_closed) >= held_conns,
        "origin held sockets were not all closed after StopProxy"
    );
    Ok(ProxyStopReport {
        held_connections: held_conns,
        stop_reclaim_ms,
        listener_closed,
        origin_active_after_stop: after.hold_active,
    })
}
