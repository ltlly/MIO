use super::*;
use mio_plugin_api::Header;
use std::io::ErrorKind;
use std::time::{Duration, Instant};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};
use tokio_util::sync::CancellationToken;

mod download;
mod raw_h1;
mod send;
mod streaming;

const PAYLOAD: &[u8] = &[0, 255, b'P'];
const RESPONSE: &[u8] = &[0, 254, b'R'];
