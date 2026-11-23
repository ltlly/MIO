//! Local loopback OAST listener with HTTP and DNS UDP capture.

use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, PoisonError, RwLock},
};

use anyhow::{Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    task::JoinHandle,
    time::{Duration, Instant, sleep_until, timeout_at},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use url::Url;

use crate::Uuid;

mod dns;
mod listener;
mod payload;

#[cfg(test)]
mod tests;

const DEFAULT_DNS_DOMAIN: &str = "oast.local";
const TOKEN_HEX_LEN: usize = 16;
const MAX_RAW_REQUEST_BYTES: usize = 64 * 1024;
const MAX_HTTP_CONCURRENCY: usize = 16;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(3);
const STOP_TIMEOUT: Duration = Duration::from_secs(3);
const SUMMARY_LIMIT: usize = 256;
const DEFAULT_SELF_HOSTED_POLL_INTERVAL_MS: u64 = 5_000;
const MIN_SELF_HOSTED_POLL_INTERVAL_MS: u64 = 250;
const BLOCKED_DEFAULT_CLOUD_DOMAINS: &[&str] = &["interact.sh", "interactsh.com"];
const HTTP_RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 2\r\nConnection: close\r\nCache-Control: no-store\r\n\r\nok";
const DNS_HEADER_LEN: usize = 12;
const DNS_LABEL_MAX: usize = 63;
const DNS_NAME_MAX: usize = 253;
const DNS_MAX_POINTER_JUMPS: usize = 16;
const DNS_QTYPE_A: u16 = 1;
const DNS_QTYPE_ANY: u16 = 255;
const DNS_QCLASS_IN: u16 = 1;
const DNS_QCLASS_ANY: u16 = 255;
const DNS_RCODE_NOERROR: u8 = 0;
const DNS_RCODE_FORMERR: u8 = 1;
const DNS_RCODE_NXDOMAIN: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OastProtocol {
    Http,
    Dns,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OastPayloadToken {
    pub token: String,
    pub label: String,
    pub http_callback_url: String,
    pub dns_qname: String,
    pub created_unix_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OastInteraction {
    pub id: Uuid,
    pub token: Option<String>,
    pub protocol: OastProtocol,
    pub peer_addr: String,
    pub summary: String,
    pub raw_request: Vec<u8>,
    pub observed_unix_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfHostedOastConfig {
    pub enabled: bool,
    pub polling_url: Option<String>,
    pub server_domain: Option<String>,
    pub authorization_header: Option<String>,
    pub poll_interval_ms: u64,
    #[serde(default)]
    pub bind_address: Option<String>,
    #[serde(default)]
    pub http_port: Option<u16>,
    #[serde(default)]
    pub dns_port: Option<u16>,
    #[serde(default)]
    pub dns_answer_ip: Option<String>,
    #[serde(default)]
    pub allow_public_cloud_domains: bool,
}

#[derive(Debug, Default)]
pub(super) struct IssuedTokenBook {
    pub(super) tokens: HashSet<String>,
    pub(super) payloads: Vec<OastPayloadToken>,
}

type IssuedTokens = Arc<RwLock<IssuedTokenBook>>;
type EventSender = mpsc::Sender<OastInteraction>;

pub struct LocalOastServer {
    pub(super) http_addr: SocketAddr,
    pub(super) dns_addr: SocketAddr,
    pub(super) dns_answer_ip: Ipv4Addr,
    pub(super) config: SelfHostedOastConfig,
    pub(super) issued_tokens: IssuedTokens,
    pub(super) cancel: CancellationToken,
    pub(super) connections: TaskTracker,
    pub(super) http_task: Option<JoinHandle<()>>,
    pub(super) dns_task: Option<JoinHandle<()>>,
}
