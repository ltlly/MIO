//! Synthetic native-GUI fixture for loopback streaming tests.
//! `server` prints its random origin port. `sse|ws --proxy 127.0.0.1:P
//! --origin 127.0.0.1:O [--expect-stop]` verifies the bytes received through MIO.
//! `next|eof|shutdown --origin 127.0.0.1:O` controls the source directly.
//! `stats --origin 127.0.0.1:O` and `http-load --proxy 127.0.0.1:P --origin 127.0.0.1:O`
//! provide bounded loopback R5 HTTP load generation and origin verification.
//! SSE emits FIRST immediately, SECOND on `next`, and clean EOF on `eof`.

use std::{
    error::Error,
    fs, io,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

#[path = "loopback_streams/client.rs"]
mod client;
#[path = "loopback_streams/source.rs"]
mod source;

use client::{client_sse, client_ws, self_test};
use source::{control, read_stats, start_source, start_source_with_limit};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
const IO_DEADLINE: Duration = Duration::from_secs(5);
const FLOW_DEADLINE: Duration = Duration::from_secs(240);
const FIRST: &[u8] = "id: 1\ndata: 第一段🙂\n\n".as_bytes();
const SECOND: &[u8] = "id: 2\ndata: 第二段\0\n\n".as_bytes();
const WS_TEXT: &str = "合成 WS🙂";
const WS_BINARY: &[u8] = &[0, 255, 128, 1, 10];
const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";

#[derive(Clone, Copy)]
enum Step {
    Next,
    Eof,
}

#[derive(Default)]
struct Source {
    active_sse: Mutex<Option<mpsc::Sender<Step>>>,
    bench_requests: AtomicUsize,
    hold_requests: AtomicUsize,
    hold_active: AtomicUsize,
    hold_closed: AtomicUsize,
    cancel: CancellationToken,
}

#[derive(Debug, PartialEq, Eq)]
struct HttpLoadArgs {
    proxy: SocketAddr,
    origin: SocketAddr,
    count: usize,
    concurrency: usize,
    out: Option<PathBuf>,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

async fn read_until(stream: &mut TcpStream, ending: &[u8], max: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while !bytes.ends_with(ending) {
        if bytes.len() >= max {
            return Err(invalid("fixture input exceeds its limit"));
        }
        bytes.push(stream.read_u8().await?);
    }
    Ok(bytes)
}

async fn head(stream: &mut TcpStream) -> Result<String> {
    Ok(String::from_utf8(
        tokio::time::timeout(FLOW_DEADLINE, read_until(stream, b"\r\n\r\n", 8192)).await??,
    )?)
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}

async fn connect(address: SocketAddr) -> Result<TcpStream> {
    let stream = tokio::time::timeout(IO_DEADLINE, TcpStream::connect(address)).await??;
    stream.set_nodelay(true)?;
    Ok(stream)
}

fn address(args: &[String], option: &str) -> Result<SocketAddr> {
    let mut matching = args.windows(2).filter(|pair| pair[0] == option);
    let address: SocketAddr = matching
        .next()
        .ok_or_else(|| invalid("explicit loopback address is required"))?[1]
        .parse()?;
    if matching.next().is_some() || address.ip() != Ipv4Addr::LOCALHOST || address.port() == 0 {
        return Err(invalid("only one explicit 127.0.0.1 nonzero port is allowed").into());
    }
    Ok(address)
}

fn option_usize(
    args: &[String],
    option: &str,
    default: usize,
    range: std::ops::RangeInclusive<usize>,
) -> Result<usize> {
    let mut matching = args.windows(2).filter(|pair| pair[0] == option);
    let Some(pair) = matching.next() else {
        return Ok(default);
    };
    if matching.next().is_some() {
        return Err(invalid("duplicate numeric option").into());
    }
    let value: usize = pair[1]
        .parse()
        .map_err(|_| invalid("invalid numeric option"))?;
    if !range.contains(&value) {
        return Err(invalid("numeric option out of allowed bounds").into());
    }
    Ok(value)
}

fn option_path(args: &[String], option: &str) -> Result<Option<PathBuf>> {
    let mut matching = args.windows(2).filter(|pair| pair[0] == option);
    let Some(pair) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() || pair[1].is_empty() {
        return Err(invalid("invalid or duplicate output path").into());
    }
    Ok(Some(PathBuf::from(&pair[1])))
}

fn parse_http_load_args(args: &[String]) -> Result<HttpLoadArgs> {
    Ok(HttpLoadArgs {
        proxy: address(args, "--proxy")?,
        origin: address(args, "--origin")?,
        count: option_usize(args, "--count", 5_000, 1..=100_000)?,
        concurrency: option_usize(args, "--concurrency", 16, 1..=64)?,
        out: option_path(args, "--out")?,
    })
}

fn bench_target(index: usize) -> (String, &'static str) {
    match index % 5 {
        0 => (format!("/bench/item-{index}"), "200 "),
        1 => (format!("/bench?id={index}&kind=script"), "200 "),
        2 => (format!("/bench?id={index}&kind=miss"), "404 "),
        _ => (format!("/bench?id={index}&tag=canary"), "200 "),
    }
}

async fn read_http_body(stream: &mut TcpStream, resp_head: &str) -> Result<Vec<u8>> {
    if header(resp_head, "transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        let mut all = Vec::new();
        loop {
            let line = read_until(stream, b"\r\n", 64).await?;
            let text = std::str::from_utf8(&line).map_err(|_| invalid("invalid chunk header"))?;
            let len = usize::from_str_radix(text.trim().split(';').next().unwrap_or(""), 16)
                .map_err(|_| invalid("invalid chunk size"))?;
            if len == 0 {
                let _ = read_until(stream, b"\r\n", 8192).await?;
                break;
            }
            if all.len() + len > 256 * 1024 {
                return Err(invalid("response chunk exceeds limit").into());
            }
            let start = all.len();
            all.resize(start + len, 0);
            stream.read_exact(&mut all[start..]).await?;
            let mut crlf = [0; 2];
            stream.read_exact(&mut crlf).await?;
            if crlf != *b"\r\n" {
                return Err(invalid("invalid chunk crlf").into());
            }
        }
        return Ok(all);
    }
    let len = header(resp_head, "content-length")
        .ok_or_else(|| invalid("missing content-length on benchmark response"))?
        .parse::<usize>()
        .map_err(|_| invalid("invalid response content-length"))?;
    if len > 256 * 1024 {
        return Err(invalid("response body exceeds limit").into());
    }
    let mut body = vec![0; len];
    if len > 0 {
        stream.read_exact(&mut body).await?;
    }
    Ok(body)
}

fn percentile_us(sorted: &[u64], numerator: usize, denominator: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (sorted.len() * numerator)
        .div_ceil(denominator)
        .saturating_sub(1);
    sorted[rank.min(sorted.len() - 1)]
}

async fn run_http_load(cfg: HttpLoadArgs) -> Result<()> {
    let (_, before_bench) = read_stats(cfg.origin).await?;
    let next_index = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();
    let mut workers = JoinSet::new();
    for _ in 0..cfg.concurrency {
        let next = next_index.clone();
        let proxy = cfg.proxy;
        let origin = cfg.origin;
        let total = cfg.count;
        workers.spawn(async move {
            let mut samples = Vec::new();
            let mut conn: Option<TcpStream> = None;
            loop {
                let idx = next.fetch_add(1, Ordering::Relaxed);
                if idx >= total {
                    break;
                }
                let (target, expected_status) = bench_target(idx);
                let raw = format!(
                    "GET http://{origin}{target} HTTP/1.1\r\nHost: {origin}\r\nConnection: keep-alive\r\n\r\n"
                );
                let req_started = Instant::now();
                let reused = conn.is_some();
                if !reused {
                    conn = Some(connect(proxy).await?);
                }
                let first_try = async {
                    let stream = conn.as_mut().expect("connected stream");
                    stream.write_all(raw.as_bytes()).await?;
                    head(stream).await
                }
                .await;
                let resp_head = match first_try {
                    Ok(h) => h,
                    Err(_) if reused => {
                        let mut fresh = connect(proxy).await?;
                        fresh.write_all(raw.as_bytes()).await?;
                        let h = head(&mut fresh).await?;
                        conn = Some(fresh);
                        h
                    }
                    Err(err) => return Err(err),
                };
                let stream = conn.as_mut().expect("connected stream");
                let status_ok = resp_head
                    .lines()
                    .next()
                    .is_some_and(|line| line.starts_with("HTTP/1.1 ") && line[9..].starts_with(expected_status));
                if !status_ok {
                    return Err(invalid("unexpected HTTP status from proxy benchmark").into());
                }
                let body = read_http_body(stream, &resp_head).await?;
                let elapsed_us = req_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
                let body_str = std::str::from_utf8(&body)
                    .map_err(|_| invalid("non-utf8 benchmark payload"))?;
                if !body_str.contains(&target) {
                    return Err(invalid("origin payload target mismatch").into());
                }
                if header(&resp_head, "connection")
                    .is_some_and(|value| value.eq_ignore_ascii_case("close"))
                {
                    conn = None;
                }
                samples.push(elapsed_us);
            }
            Ok::<Vec<u64>, Box<dyn Error + Send + Sync>>(samples)
        });
    }
    let mut latencies = Vec::with_capacity(cfg.count);
    while let Some(joined) = workers.join_next().await {
        latencies.extend(joined??);
    }
    let wall = started.elapsed();
    let (_, after_bench) = read_stats(cfg.origin).await?;
    let origin_delta = after_bench.saturating_sub(before_bench);
    if latencies.len() != cfg.count || origin_delta != cfg.count {
        return Err(
            invalid("completed or origin-observed count did not match requested load").into(),
        );
    }
    latencies.sort_unstable();
    let sum_us: u128 = latencies.iter().map(|&v| u128::from(v)).sum();
    let mean_us = (sum_us / (latencies.len() as u128)) as u64;
    let wall_ms = wall.as_secs_f64() * 1000.0;
    let rps = (cfg.count as f64) / wall.as_secs_f64().max(1e-9);
    let json = format!(
        "{{\n  \"proxy\": \"{}\",\n  \"origin\": \"{}\",\n  \"requests\": {},\n  \"origin_observed\": {},\n  \"concurrency\": {},\n  \"wall_ms\": {:.3},\n  \"throughput_rps\": {:.2},\n  \"latency_us\": {{\n    \"min\": {},\n    \"p50\": {},\n    \"p95\": {},\n    \"p99\": {},\n    \"max\": {},\n    \"mean\": {}\n  }}\n}}\n",
        cfg.proxy,
        cfg.origin,
        cfg.count,
        origin_delta,
        cfg.concurrency,
        wall_ms,
        rps,
        latencies[0],
        percentile_us(&latencies, 50, 100),
        percentile_us(&latencies, 95, 100),
        percentile_us(&latencies, 99, 100),
        *latencies.last().unwrap(),
        mean_us,
    );
    if let Some(out) = &cfg.out {
        fs::write(out, &json)?;
    }
    print!("{json}");
    Ok(())
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("server") => {
            let max_conns = option_usize(&args[1..], "--max-conns", 16, 1..=256)?;
            let quiet = args[1..].iter().any(|arg| arg == "--quiet");
            let (address, _cancel, source) = if max_conns == 16 && !quiet {
                start_source().await?
            } else {
                start_source_with_limit(max_conns, !quiet).await?
            };
            println!("READY origin_port={}", address.port());
            source.await??;
        }
        Some("http-load") => {
            let cfg = parse_http_load_args(&args[1..])?;
            tokio::time::timeout(Duration::from_secs(540), run_http_load(cfg)).await??;
        }
        Some("stats") => {
            let (raw_json, _) =
                tokio::time::timeout(IO_DEADLINE, read_stats(address(&args, "--origin")?))
                    .await??;
            print!("{raw_json}");
        }
        Some("sse" | "ws") => {
            let proxy = address(&args, "--proxy")?;
            let origin = address(&args, "--origin")?;
            let stop = args.iter().any(|arg| arg == "--expect-stop");
            tokio::time::timeout(FLOW_DEADLINE, async {
                if args[0] == "sse" {
                    client_sse(proxy, origin, stop, None).await
                } else {
                    client_ws(proxy, origin, stop, None).await
                }
            })
            .await??;
        }
        Some(command @ ("next" | "eof" | "shutdown")) => {
            tokio::time::timeout(IO_DEADLINE, control(address(&args, "--origin")?, command))
                .await??;
        }
        Some("self-test") => {
            self_test().await?;
        }
        _ => {
            return Err(invalid(
                "use server [--max-conns N] [--quiet], http-load --proxy 127.0.0.1:P --origin 127.0.0.1:O [--count N] [--concurrency N] [--out FILE], stats --origin 127.0.0.1:O, sse|ws --proxy 127.0.0.1:P --origin 127.0.0.1:O [--expect-stop], next|eof|shutdown --origin 127.0.0.1:O, or self-test",
            )
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn synthetic_streams_check_real_bytes_and_join_every_socket() {
        self_test().await.unwrap();
    }

    #[test]
    fn explicit_addresses_never_allow_external_hosts_or_wildcard_listeners() {
        for host in [
            "0.0.0.0:1234",
            "127.0.0.2:1234",
            "1.1.1.1:1234",
            "[::1]:1234",
            "127.0.0.1:0",
        ] {
            assert!(address(&["--proxy".into(), host.into()], "--proxy").is_err());
        }
        assert!(address(&["--proxy".into(), "127.0.0.1:1234".into()], "--proxy").is_ok());
    }

    #[test]
    fn http_load_cli_parses_defaults_and_rejects_out_of_range_values() {
        let parsed = parse_http_load_args(&[
            "--proxy".into(),
            "127.0.0.1:8080".into(),
            "--origin".into(),
            "127.0.0.1:9090".into(),
        ])
        .unwrap();
        assert_eq!(parsed.count, 5_000);
        assert_eq!(parsed.concurrency, 16);
        assert_eq!(parsed.out, None);
        assert!(
            parse_http_load_args(&[
                "--proxy".into(),
                "127.0.0.1:8080".into(),
                "--origin".into(),
                "127.0.0.1:9090".into(),
                "--concurrency".into(),
                "0".into(),
            ])
            .is_err()
        );
    }

    #[tokio::test]
    async fn http_load_verifies_bench_variants_and_origin_stats_on_loopback() {
        let (events, _captures) = mpsc::channel(32);
        let mut proxy = mio_proxy::start(
            mio_proxy::ProxyConfig::new(mio_proxy::MitmAuthority::generate().unwrap()),
            mio_plugin_api::ProjectKey {
                id: mio_plugin_api::Uuid::new_v4(),
                generation: 1,
            },
            events,
        )
        .await
        .unwrap();
        let (origin, cancel, source) = start_source_with_limit(24, false).await.unwrap();
        let load_res = run_http_load(HttpLoadArgs {
            proxy: proxy.address(),
            origin,
            count: 5,
            concurrency: 2,
            out: None,
        })
        .await;
        let stop_res = proxy.stop().await;
        let _ = control(origin, "shutdown").await;
        cancel.cancel();
        let joined = source.await;
        load_res.unwrap();
        stop_res.unwrap();
        joined.unwrap().unwrap();
    }
}
