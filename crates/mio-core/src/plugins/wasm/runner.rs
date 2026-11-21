use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use mio_plugin_api::protocol::{
    self, ChannelIdentity, Envelope, ErrorCode, Failure, envelope, host_result,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{
    WASM_CALL_FUEL, WasmHostFn, WasmInstance, WasmModule,
    legacy::{handle_legacy_wasm_body, parse_legacy_host_call},
};

pub fn dispatch_wasm_envelope(
    vm: &mut WasmInstance,
    envelope: &Envelope,
    fuel: u64,
    cancel: &CancellationToken,
    host: WasmHostFn<'_>,
) -> Result<Option<envelope::Body>> {
    let encoded = envelope.encode_bytes();
    ensure!(
        encoded.len() <= protocol::MAX_FRAME_BYTES,
        "Wasm 输入帧超过 1 MiB 限额"
    );
    let alloc = vm
        .invoke(
            "mio_alloc",
            &[i64::try_from(encoded.len())?],
            fuel,
            cancel,
            host,
        )?
        .context("mio_alloc 未返回指针")? as i32;
    let in_ptr = usize::try_from(alloc).context("mio_alloc 返回非法指针")?;
    vm.write_memory(in_ptr, &encoded)?;
    let packed = vm
        .invoke(
            "mio_dispatch",
            &[in_ptr as i64, encoded.len() as i64],
            fuel,
            cancel,
            host,
        )?
        .context("mio_dispatch 未返回结果")? as u64;
    let (out_ptr, out_len) = (((packed >> 32) as u32) as usize, (packed as u32) as usize);
    if out_len == 0 {
        return Ok(None);
    }
    ensure!(
        out_len <= protocol::MAX_FRAME_BYTES,
        "Wasm 返回帧超过 1 MiB 限额"
    );
    Ok(Envelope::decode_bytes(vm.read_memory(out_ptr, out_len)?)
        .map_err(anyhow::Error::msg)?
        .body)
}

pub(crate) fn spawn_sandbox_worker(
    identity: ChannelIdentity,
    plugin_id: String,
    module: WasmModule,
    mut writes: mpsc::Receiver<Envelope>,
    wire: mpsc::Sender<super::Wire>,
    outbound_claims: std::sync::Arc<crate::plugins::OutboundWriteClaims>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let (sync_tx, sync_rx) = std::sync::mpsc::sync_channel::<Option<Envelope>>(64);
    let bridge_cancel = cancel.clone();
    tokio::spawn(async move {
        loop {
            let msg = tokio::select! {
                biased;
                _ = bridge_cancel.cancelled() => None,
                next = writes.recv() => next,
            };
            let done = msg.is_none();
            if sync_tx.send(msg).is_err() || done {
                break;
            }
        }
    });
    tokio::task::spawn_blocking(move || {
        let Ok(mut vm) = WasmInstance::new(module) else {
            let _ = wire.blocking_send(super::Wire::Failed("无法实例化受限 Wasm 模块"));
            return;
        };
        let mut seq = 1u64;
        let mut panel_rev = 1u64;
        let mut deferred = std::collections::VecDeque::new();
        let send_env =
            |seq: &mut u64, rev: u64, reply_to: u64, body: envelope::Body| -> Result<u64> {
                *seq = seq.checked_add(1).context("Wasm 序号耗尽")?;
                let mut env = Envelope::new(identity, *seq, rev, body, Duration::from_secs(5))
                    .with_minor(protocol::LEGACY_HANDSHAKE_MINOR);
                env.in_reply_to = reply_to;
                wire.blocking_send(super::Wire::Frame(Box::new(env)))
                    .map_err(|_| anyhow::anyhow!("Wasm 事件通道已关闭"))?;
                Ok(*seq)
            };
        loop {
            if cancel.is_cancelled() {
                break;
            }
            let next = if let Some(msg) = deferred.pop_front() {
                Some(msg)
            } else {
                match sync_rx.recv() {
                    Ok(Some(msg)) => Some(msg),
                    _ => None,
                }
            };
            let Some(env) = next else { break };
            if !outbound_claims.claim_for_write(env.panel_id, env.request_id) {
                continue;
            }
            let req_id = env.request_id;
            let Some(body) = env.body.clone() else {
                continue;
            };
            if matches!(body, envelope::Body::Shutdown(_)) {
                break;
            }
            if matches!(body, envelope::Body::Cancel(_)) {
                continue;
            }
            if let envelope::Body::Resync(ref snap) = body {
                panel_rev = snap.revision;
            }
            let current_rev = panel_rev;
            let mut host_bridge = |raw_req: &[u8],
                                   out: &mut [u8],
                                   envelope_abi: bool,
                                   tok: &CancellationToken|
             -> Result<usize> {
                ensure!(!tok.is_cancelled(), "Wasm HostCall 已取消");
                let call = if envelope_abi {
                    protocol::HostCall::decode_bytes(raw_req).map_err(anyhow::Error::msg)?
                } else {
                    parse_legacy_host_call(raw_req)?
                };
                let call_id = send_env(&mut seq, current_rev, 0, envelope::Body::HostCall(call))?;
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    ensure!(!tok.is_cancelled(), "Wasm HostCall 已取消");
                    let rem = deadline.saturating_duration_since(Instant::now());
                    ensure!(!rem.is_zero(), "Wasm HostCall 超时");
                    let incoming = sync_rx
                        .recv_timeout(rem)
                        .context("等待宿主结果超时或取消")?
                        .context("Wasm 沙箱正在停止")?;
                    match incoming.body {
                        Some(envelope::Body::Shutdown(_)) => bail!("Wasm 沙箱正在关闭"),
                        Some(envelope::Body::Cancel(c)) if c.request_id == req_id => {
                            bail!("Wasm 当前操作已取消")
                        }
                        Some(envelope::Body::HostResult(res))
                            if incoming.in_reply_to == call_id =>
                        {
                            if envelope_abi {
                                if let Some(host_result::Outcome::Error(f)) = &res.outcome {
                                    bail!(f.message.clone());
                                }
                                let bytes = res.encode_bytes();
                                ensure!(bytes.len() <= out.len(), "输出缓冲不足");
                                out[..bytes.len()].copy_from_slice(&bytes);
                                return Ok(bytes.len());
                            }
                            return match res.outcome.context("空宿主结果")? {
                                host_result::Outcome::Error(f) => bail!(f.message),
                                host_result::Outcome::Done(_) => Ok(0),
                                host_result::Outcome::StorageValue(v) => {
                                    if !v.present {
                                        Ok(0)
                                    } else {
                                        ensure!(v.value.len() <= out.len(), "输出缓冲不足");
                                        out[..v.value.len()].copy_from_slice(&v.value);
                                        Ok(v.value.len())
                                    }
                                }
                                host_result::Outcome::Response(r) => {
                                    let n = 2 + r.body.len().min(out.len().saturating_sub(2));
                                    ensure!(out.len() >= 2, "输出缓冲不足");
                                    out[..2].copy_from_slice(&(r.status as u16).to_le_bytes());
                                    out[2..n].copy_from_slice(&r.body[..n - 2]);
                                    Ok(n)
                                }
                                other => {
                                    let bytes = protocol::HostResult {
                                        outcome: Some(other),
                                    }
                                    .encode_bytes();
                                    ensure!(bytes.len() <= out.len(), "输出缓冲不足");
                                    out[..bytes.len()].copy_from_slice(&bytes);
                                    Ok(bytes.len())
                                }
                            };
                        }
                        _ => deferred.push_back(incoming),
                    }
                }
            };
            let outcome = if vm.m.is_envelope_abi() {
                dispatch_wasm_envelope(&mut vm, &env, WASM_CALL_FUEL, &cancel, &mut host_bridge)
                    .inspect(|opt| {
                        if let Some(envelope::Body::Snapshot(s)) = opt {
                            panel_rev = s.revision;
                        } else if let Some(envelope::Body::Patch(p)) = opt {
                            panel_rev = p.revision;
                        }
                    })
            } else {
                handle_legacy_wasm_body(
                    &plugin_id,
                    &mut vm,
                    &mut panel_rev,
                    body,
                    &cancel,
                    &mut host_bridge,
                )
            };
            let reply_body = match outcome {
                Ok(Some(b)) => b,
                Ok(None) => continue,
                Err(err) => envelope::Body::Error(Failure::new(
                    ErrorCode::Invalid,
                    err.to_string().chars().take(1024).collect::<String>(),
                )),
            };
            if send_env(&mut seq, panel_rev, req_id, reply_body).is_err() {
                break;
            }
        }
    })
}
