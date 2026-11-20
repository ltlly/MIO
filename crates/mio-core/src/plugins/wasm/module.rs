use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::Result;
use mio_plugin_api::{
    ProjectKey, Uuid,
    panel::{self, Button, Container, Editor, EditorFormat, PanelSnapshot, Text, node},
    protocol::{self, ChannelIdentity, Envelope, Hello, Permission, envelope},
};
use tokio_util::sync::CancellationToken;

use crate::plugins::PluginManifest;

use super::{I32, MAGIC, WASM_CALL_FUEL, WasmInstance, WasmModule, dispatch_wasm_envelope};

fn find_node_mut(node: &mut panel::Node, id: u64) -> Option<&mut panel::Node> {
    if node.id == id {
        return Some(node);
    }
    match &mut node.kind {
        Some(node::Kind::Row(c) | node::Kind::Column(c) | node::Kind::ScrollColumn(c)) => {
            c.children.iter_mut().find_map(|n| find_node_mut(n, id))
        }
        Some(node::Kind::Split(s)) => s
            .first
            .as_deref_mut()
            .and_then(|n| find_node_mut(n, id))
            .or_else(|| s.second.as_deref_mut().and_then(|n| find_node_mut(n, id))),
        Some(node::Kind::Tabs(t)) => t.tabs.iter_mut().find_map(|tab| {
            tab.content
                .as_deref_mut()
                .and_then(|n| find_node_mut(n, id))
        }),
        _ => None,
    }
}

pub fn initial_wasm_panel(
    manifest: &PluginManifest,
    module: &WasmModule,
    grants: &BTreeSet<Permission>,
) -> Result<PanelSnapshot> {
    let status = format!("{} (wasm) 受限沙箱运行中", manifest.id);
    if !module.is_envelope_abi() {
        return Ok(default_sandbox_panel(&manifest.id, &status, 1));
    }
    let can_storage = grants.contains(&Permission::Storage);
    if let Ok(mut vm) = WasmInstance::new(module.clone()) {
        let id = ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            },
        };
        let hello = Envelope::new(
            id,
            1,
            0,
            envelope::Body::Hello(Hello {
                max_frame_bytes: protocol::MAX_FRAME_BYTES as u32,
                max_operation_ms: 5_000,
                granted_permissions: grants.iter().map(|p| *p as i32).collect(),
            }),
            Duration::from_secs(5),
        );
        let cancel = CancellationToken::new();
        let mut noop = |_: &[u8], _: &mut [u8], _: bool, _: &CancellationToken| Ok(0);
        if let Ok(Some(envelope::Body::Snapshot(mut snap))) =
            dispatch_wasm_envelope(&mut vm, &hello, WASM_CALL_FUEL, &cancel, &mut noop)
        {
            snap.title = manifest.id.clone();
            if let Some(root) = &mut snap.root {
                if let Some(node::Kind::Button(btn)) =
                    find_node_mut(root, 10).and_then(|n| n.kind.as_mut())
                {
                    btn.enabled = can_storage;
                }
                if let Some(node::Kind::Text(txt)) =
                    find_node_mut(root, 3).and_then(|n| n.kind.as_mut())
                {
                    txt.text = status.clone();
                }
            }
            if snap.validate().is_ok() {
                return Ok(snap);
            }
        }
    }
    Ok(PanelSnapshot {
        revision: 1,
        title: manifest.id.clone(),
        root: Some(panel::Node::new(
            1,
            "Envelope 沙箱工作台",
            node::Kind::Column(Container {
                children: vec![
                    panel::Node::new(
                        2,
                        "操作栏",
                        node::Kind::Row(Container {
                            children: vec![panel::Node::new(
                                10,
                                "写入全局偏好",
                                node::Kind::Button(Button {
                                    label: "写入全局偏好".into(),
                                    enabled: can_storage,
                                    commit_editors: false,
                                }),
                            )],
                        }),
                    ),
                    panel::Node::new(
                        3,
                        "状态",
                        node::Kind::Text(Text {
                            text: status,
                            tone: panel::Tone::Foreground as i32,
                        }),
                    ),
                    panel::Node::new(
                        30,
                        "消息载荷",
                        node::Kind::Editor(Editor {
                            document_id: 100,
                            revision: 1,
                            contents: vec![],
                            format: EditorFormat::Binary as i32,
                            read_only: false,
                        }),
                    ),
                ],
            }),
        )),
    })
}

pub fn default_sandbox_panel(plugin_id: &str, status: &str, revision: u64) -> PanelSnapshot {
    PanelSnapshot {
        revision,
        title: plugin_id.to_owned(),
        root: Some(panel::Node::new(
            1,
            "沙箱工作台",
            node::Kind::Column(Container {
                children: vec![
                    panel::Node::new(
                        2,
                        "操作栏",
                        node::Kind::Row(Container {
                            children: vec![
                                panel::Node::new(
                                    10,
                                    "沙箱写入 KV",
                                    node::Kind::Button(Button {
                                        label: "沙箱写入 KV".into(),
                                        enabled: true,
                                        commit_editors: false,
                                    }),
                                ),
                                panel::Node::new(
                                    11,
                                    "沙箱读取 KV",
                                    node::Kind::Button(Button {
                                        label: "沙箱读取 KV".into(),
                                        enabled: true,
                                        commit_editors: false,
                                    }),
                                ),
                            ],
                        }),
                    ),
                    panel::Node::new(
                        3,
                        "状态",
                        node::Kind::Text(Text {
                            text: status.to_owned(),
                            tone: panel::Tone::Foreground as i32,
                        }),
                    ),
                ],
            }),
        )),
    }
}

pub(super) fn push_uleb(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let mut b = (n & 0x7f) as u8;
        n >>= 7;
        if n != 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
}

pub(super) fn push_sleb(mut v: i32, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        let done = (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0);
        out.push(if done { b } else { b | 0x80 });
        if done {
            break;
        }
    }
}

pub(super) fn push_section(id: u8, body: &[u8], out: &mut Vec<u8>) {
    out.push(id);
    push_uleb(body.len(), out);
    out.extend_from_slice(body);
}

/// Builds a deterministic, zero-WASI sandboxed `.wasm` plugin supporting HTTP/WS/Session/Payload hooks and KV host calls.
pub fn build_demo_wasm_module() -> Vec<u8> {
    let mut wasm = MAGIC.to_vec();
    let types = [
        3u8, // 3 function signatures
        0x60, 4, I32, I32, I32, I32, 1, I32, // 0: mio_host_call
        0x60, 3, I32, I32, I32, 0, // 1: copy_bytes(src, dst, len)
        0x60, 5, I32, I32, I32, I32, I32, 1, I32, // 2: mio_transform
    ];
    push_section(1, &types, &mut wasm);
    push_section(2, b"\x01\x03mio\x0dmio_host_call\x00\x00", &mut wasm);
    push_section(3, &[2, 1, 2], &mut wasm);
    push_section(5, &[1, 1, 2, 16], &mut wasm);
    let mut exports = vec![2u8];
    exports.extend_from_slice(b"\x06memory\x02\x00");
    exports.extend_from_slice(b"\x0dmio_transform\x00\x02");
    push_section(7, &exports, &mut wasm);

    // Func 1: copy_bytes(src=0, dst=1, len=2), local 3 = i
    let f1_ops: &[u8] = &[
        1, 1, I32, // 1 local i32
        0x02, 0x40, 0x03, 0x40, // block, loop
        0x20, 3, 0x20, 2, 0x4f, 0x0d, 1, // br_if 1 (i >= len)
        0x20, 1, 0x20, 3, 0x6a, // dst + i
        0x20, 0, 0x20, 3, 0x6a, 0x2d, 0, 0, // load8_u(src + i)
        0x3a, 0, 0, // store8
        0x20, 3, 0x41, 1, 0x6a, 0x21, 3, // i += 1
        0x0c, 0, // br 0
        0x0b, 0x0b, 0x0b,
    ];

    // Func 2: mio_transform(op=0, in_ptr=1, in_len=2, out_ptr=3, out_cap=4) -> i32
    let mut f2_ops = vec![1u8, 1, I32]; // local 5 = node_id
    let emit_const_copy = |f: &mut Vec<u8>, op_val: i32, addr: i32, len: i32| {
        f.extend_from_slice(&[0x20, 0, 0x41]);
        push_sleb(op_val, f);
        f.extend_from_slice(&[0x46, 0x04, 0x40, 0x41]); // if op == op_val
        push_sleb(addr, f);
        f.extend_from_slice(&[0x20, 3, 0x41]);
        push_sleb(len, f);
        f.extend_from_slice(&[0x10, 1, 0x41]); // call copy_bytes; const len
        push_sleb(len, f);
        f.extend_from_slice(&[0x0f, 0x0b]); // return; end
    };
    emit_const_copy(&mut f2_ops, 1, 0x100, 26);
    emit_const_copy(&mut f2_ops, 2, 0x120, 25);
    emit_const_copy(&mut f2_ops, 4, 0x150, 28);
    emit_const_copy(&mut f2_ops, 5, 0x170, 32);
    for (op_val, pref_addr, pref_len) in [(3i32, 0x140i32, 7i32), (6, 0x198, 5)] {
        f2_ops.extend_from_slice(&[0x20, 0, 0x41]);
        push_sleb(op_val, &mut f2_ops);
        f2_ops.extend_from_slice(&[0x46, 0x04, 0x40, 0x41]);
        push_sleb(pref_addr, &mut f2_ops);
        f2_ops.extend_from_slice(&[0x20, 3, 0x41]);
        push_sleb(pref_len, &mut f2_ops);
        f2_ops.extend_from_slice(&[0x10, 1, 0x20, 1, 0x20, 3, 0x41]); // copy prefix; in_ptr, out_ptr + pref_len
        push_sleb(pref_len, &mut f2_ops);
        f2_ops.extend_from_slice(&[0x6a, 0x20, 2, 0x10, 1, 0x20, 2, 0x41]); // add, in_len, call copy; in_len + pref_len
        push_sleb(pref_len, &mut f2_ops);
        f2_ops.extend_from_slice(&[0x6a, 0x0f, 0x0b]); // add, return, end
    }
    // op == 7 (UI event button node_id at in_ptr)
    f2_ops.extend_from_slice(&[
        0x20, 0, 0x41, 7, 0x46, 0x04, 0x40, // if op == 7
        0x20, 1, 0x28, 0, 0, 0x21, 5, // local 5 = load32(in_ptr)
        // node_id == 10 => StorageSet + status
        0x20, 5, 0x41, 10, 0x46, 0x04, 0x40, 0x41,
    ]);
    push_sleb(0x1a0, &mut f2_ops);
    f2_ops.extend_from_slice(&[0x41, 23, 0x20, 3, 0x20, 4, 0x10, 0, 0x1a, 0x41]);
    push_sleb(0x1d0, &mut f2_ops);
    f2_ops.extend_from_slice(&[0x20, 3, 0x41, 24, 0x10, 1, 0x41, 24, 0x0f, 0x0b]);
    // node_id == 11 => StorageGet -> out_ptr
    f2_ops.extend_from_slice(&[0x20, 5, 0x41, 11, 0x46, 0x04, 0x40, 0x41]);
    push_sleb(0x1c0, &mut f2_ops);
    f2_ops.extend_from_slice(&[0x41, 10, 0x20, 3, 0x20, 4, 0x10, 0, 0x0f, 0x0b]);
    f2_ops.push(0x0b); // end op == 7
    f2_ops.extend_from_slice(&[0x41, 0, 0x0b]); // default 0; end func

    let mut code = vec![2u8];
    push_uleb(f1_ops.len(), &mut code);
    code.extend_from_slice(f1_ops);
    push_uleb(f2_ops.len(), &mut code);
    code.extend_from_slice(&f2_ops);
    push_section(10, &code, &mut wasm);

    let segments: &[(i32, &[u8])] = &[
        (0x100, b"x-mio-wasm-hook: sandboxed"),
        (0x120, b"x-mio-wasm-resp: verified"),
        (0x140, b"[wasm] "),
        (0x150, b"x-mio-wasm-session: token-v1"),
        (0x170, b"wasm-gen-1\nwasm-gen-2\nwasm-gen-3"),
        (0x198, b"wasm:"),
        (0x1a0, b"\x02\x09wasm_pingok-from-wasm"),
        (0x1c0, b"\x01wasm_ping"),
        (0x1d0, b"KV saved: ok-from-wasm!!"),
    ];
    let mut data = Vec::new();
    push_uleb(segments.len(), &mut data);
    for &(off, bytes) in segments {
        data.extend_from_slice(&[0, 0x41]);
        push_sleb(off, &mut data);
        data.push(0x0b);
        push_uleb(bytes.len(), &mut data);
        data.extend_from_slice(bytes);
    }
    push_section(11, &data, &mut wasm);
    wasm
}

/// Builds a zero-WASI Envelope ABI `.wasm` plugin (`mio_alloc` + `mio_dispatch` + `mio_host::host_call`).
pub fn build_envelope_wasm_module() -> Vec<u8> {
    super::envelope_module::build()
}
