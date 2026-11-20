use anyhow::{Context, Result, bail, ensure};
use mio_plugin_api::{
    panel,
    protocol::{
        self, HeaderDto, HookAction, HostCall, HttpHookResult, PayloadOp, PayloadResult,
        SessionActionResult, StorageEntry, WebSocketHookResult, envelope, host_call,
    },
};
use tokio_util::sync::CancellationToken;

use super::{SCRATCH_BASE, WASM_CALL_FUEL, WasmHostFn, WasmInstance, default_sandbox_panel};

pub(super) fn apply_header_lines(headers: &mut Vec<HeaderDto>, raw_lines: &[u8]) -> Result<bool> {
    let text = std::str::from_utf8(raw_lines).context("Wasm 返回的 header 行不是有效 UTF-8")?;
    let mut changed = false;
    for line in text.split('\n').map(|s| s.trim_end_matches('\r')) {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .context("Wasm 返回的 header 缺少冒号")?;
        let name = name.trim();
        let value = value.trim().as_bytes().to_vec();
        ensure!(
            !name.is_empty() && name.len() <= 256,
            "Wasm header 名称无效"
        );
        headers.retain(|h| !h.name.eq_ignore_ascii_case(name));
        headers.push(HeaderDto {
            name: name.to_owned(),
            value,
        });
        changed = true;
    }
    Ok(changed)
}

pub(super) fn parse_legacy_host_call(req: &[u8]) -> Result<HostCall> {
    match req.first().copied() {
        Some(1) => {
            let key = std::str::from_utf8(&req[1..]).context("Wasm KV key 非 UTF-8")?;
            Ok(HostCall {
                operation: Some(host_call::Operation::StorageGet(key.to_owned())),
            })
        }
        Some(2) => {
            ensure!(req.len() >= 2, "Wasm StorageSet 参数不足");
            let klen = req[1] as usize;
            ensure!(req.len() >= 2 + klen, "Wasm StorageSet key 越界");
            let key = std::str::from_utf8(&req[2..2 + klen]).context("Wasm KV key 非 UTF-8")?;
            Ok(HostCall {
                operation: Some(host_call::Operation::StorageSet(StorageEntry {
                    key: key.to_owned(),
                    value: req[2 + klen..].to_vec(),
                })),
            })
        }
        Some(3) => {
            let url = std::str::from_utf8(&req[1..]).context("Wasm URL 非 UTF-8")?;
            Ok(HostCall {
                operation: Some(host_call::Operation::NetworkSend(protocol::RequestDto {
                    method: "GET".into(),
                    url: url.to_owned(),
                    version: protocol::HttpVersionDto::Http11 as i32,
                    headers: vec![],
                    body: vec![],
                    trailers: vec![],
                })),
            })
        }
        _ => protocol::HostCall::decode_bytes(req).map_err(anyhow::Error::msg),
    }
}

pub(in crate::plugins::wasm) fn run_wasm_op(
    vm: &mut WasmInstance,
    op: i32,
    input: &[u8],
    out_cap: usize,
    cancel: &CancellationToken,
    host: WasmHostFn<'_>,
) -> Result<(i32, Vec<u8>)> {
    if !vm.m.has_export("mio_transform") {
        return Ok((0, Vec::new()));
    }
    let in_ptr = SCRATCH_BASE;
    let out_ptr = (in_ptr + input.len() + 16) & !15;
    let total_needed = out_ptr
        .checked_add(out_cap)
        .context("Wasm 缓冲区计算溢出")?;
    vm.ensure_memory(total_needed)?;
    vm.write_memory(in_ptr, input)?;
    let code = vm
        .invoke(
            "mio_transform",
            &[
                i64::from(op),
                in_ptr as i64,
                input.len() as i64,
                out_ptr as i64,
                out_cap as i64,
            ],
            WASM_CALL_FUEL,
            cancel,
            host,
        )?
        .unwrap_or(0) as i32;
    if code > 0 {
        let len = usize::try_from(code).context("Wasm 返回长度非法")?;
        ensure!(len <= out_cap, "Wasm 返回长度超过输出上限");
        Ok((code, vm.read_memory(out_ptr, len)?.to_vec()))
    } else {
        Ok((code, Vec::new()))
    }
}

pub(super) fn handle_legacy_wasm_body(
    plugin_id: &str,
    vm: &mut WasmInstance,
    panel_rev: &mut u64,
    body: envelope::Body,
    cancel: &CancellationToken,
    host: WasmHostFn<'_>,
) -> Result<Option<envelope::Body>> {
    match body {
        envelope::Body::HttpHookCall(call) => {
            let stage = if call.response_stage { 2 } else { 1 };
            let input = call
                .request
                .as_ref()
                .map(|r| format!("{} {}", r.method, r.url).into_bytes())
                .unwrap_or_default();
            let (code, out) = run_wasm_op(vm, stage, &input, 16 * 1024, cancel, host)?;
            let res = match code {
                0 => HttpHookResult {
                    action: HookAction::Pass as i32,
                    request: None,
                    response: None,
                },
                -1 => HttpHookResult {
                    action: HookAction::Drop as i32,
                    request: None,
                    response: None,
                },
                -2 => HttpHookResult {
                    action: HookAction::Intercept as i32,
                    request: None,
                    response: None,
                },
                n if n > 0 && !call.response_stage => {
                    let mut req = call.request.context("缺少请求")?;
                    let changed = apply_header_lines(&mut req.headers, &out)?;
                    HttpHookResult {
                        action: if changed {
                            HookAction::Modify
                        } else {
                            HookAction::Pass
                        } as i32,
                        request: changed.then_some(req),
                        response: None,
                    }
                }
                n if n > 0 => {
                    let mut resp = call.response.context("缺少响应")?;
                    let changed = apply_header_lines(&mut resp.headers, &out)?;
                    HttpHookResult {
                        action: if changed {
                            HookAction::Modify
                        } else {
                            HookAction::Pass
                        } as i32,
                        request: None,
                        response: changed.then_some(resp),
                    }
                }
                other => bail!("Wasm HttpHook 返回未知状态码 {other}"),
            };
            Ok(Some(envelope::Body::HttpHookResult(res)))
        }
        envelope::Body::WebSocketHookCall(call) => {
            let cap = (call.payload.len() + 4096).min(protocol::MAX_INLINE_BODY);
            let (code, out) = run_wasm_op(vm, 3, &call.payload, cap, cancel, host)?;
            let res = match code {
                0 => WebSocketHookResult {
                    action: HookAction::Pass as i32,
                    payload: vec![],
                },
                -1 => WebSocketHookResult {
                    action: HookAction::Drop as i32,
                    payload: vec![],
                },
                -2 => WebSocketHookResult {
                    action: HookAction::Intercept as i32,
                    payload: call.payload,
                },
                n if n > 0 => WebSocketHookResult {
                    action: HookAction::Modify as i32,
                    payload: out,
                },
                other => bail!("Wasm WebSocketHook 返回未知状态码 {other}"),
            };
            Ok(Some(envelope::Body::WebSocketHookResult(res)))
        }
        envelope::Body::SessionActionCall(call) => {
            let mut req = call.request.context("缺少请求")?;
            let (code, out) = run_wasm_op(vm, 4, req.url.as_bytes(), 16 * 1024, cancel, host)?;
            if code > 0 && apply_header_lines(&mut req.headers, &out)? {
                Ok(Some(envelope::Body::SessionActionResult(
                    SessionActionResult {
                        modified: true,
                        request: Some(req),
                    },
                )))
            } else {
                Ok(Some(envelope::Body::SessionActionResult(
                    SessionActionResult {
                        modified: false,
                        request: None,
                    },
                )))
            }
        }
        envelope::Body::PayloadCall(call) => {
            let op = PayloadOp::try_from(call.op).map_err(|_| anyhow::anyhow!("未知 PayloadOp"))?;
            let mut outputs = Vec::new();
            match op {
                PayloadOp::Generate => {
                    let (code, out) =
                        run_wasm_op(vm, 5, call.config.as_bytes(), 64 * 1024, cancel, host)?;
                    if code > 0 {
                        for line in out.split(|b| *b == b'\n') {
                            let item = line.strip_suffix(b"\r").unwrap_or(line);
                            if !item.is_empty() && outputs.len() < call.max_items as usize {
                                outputs.push(item.to_vec());
                            }
                        }
                    }
                }
                PayloadOp::Process => {
                    let mut total = 0usize;
                    for item in &call.inputs {
                        let cap = (item.len() + 1024).min(64 * 1024);
                        let (code, out) = run_wasm_op(vm, 6, item, cap, cancel, host)?;
                        let next = if code > 0 { out } else { item.clone() };
                        total = total.saturating_add(next.len());
                        ensure!(
                            total <= protocol::MAX_INLINE_BODY,
                            "Wasm Payload 输出总字节超限"
                        );
                        outputs.push(next);
                    }
                }
            }
            Ok(Some(envelope::Body::PayloadResult(PayloadResult {
                outputs,
            })))
        }
        envelope::Body::UiEvent(ev) => {
            let node_bytes = (ev.node_id as u32).to_le_bytes();
            let (code, out) = run_wasm_op(vm, 7, &node_bytes, 4096, cancel, host)?;
            if code > 0 {
                *panel_rev = panel_rev.saturating_add(1);
                let status = String::from_utf8_lossy(&out).into_owned();
                Ok(Some(envelope::Body::Snapshot(default_sandbox_panel(
                    plugin_id, &status, *panel_rev,
                ))))
            } else {
                Ok(Some(envelope::Body::Ack(panel::Empty {})))
            }
        }
        envelope::Body::CloseRequest(_) => Ok(Some(envelope::Body::Ack(panel::Empty {}))),
        _ => Ok(None),
    }
}
