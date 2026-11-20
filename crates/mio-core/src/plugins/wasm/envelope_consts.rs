use mio_plugin_api::{
    panel::{self, Button, Container, Editor, EditorFormat, PanelSnapshot, Text, node},
    protocol::{
        ContextMenuResult, Envelope, ErrorCode, Failure, HeaderDto, HostCall, HttpHookResult,
        PassiveCheckResult, PayloadResult, PluginFindingDto, RequestDto, ResponseDto,
        SessionActionResult, StorageEntry, envelope, host_call::Operation,
    },
};

use super::envelope_ops::HEAP_BASE;

pub(super) const CONST_BASE: i32 = 0x0900;

pub(super) struct Consts {
    pub(super) hello_snap: (i32, i32),
    pub(super) ack: (i32, i32),
    pub(super) err_inv: (i32, i32),
    pub(super) pref_call: (i32, i32),
    pub(super) op_pref: (i32, i32),
    pub(super) op_open: (i32, i32),
    pub(super) n30_hdr: (i32, i32),
    pub(super) hdr_req: (i32, i32),
    pub(super) hdr_resp: (i32, i32),
    pub(super) ws_pref: (i32, i32),
    pub(super) hdr_sess: (i32, i32),
    pub(super) wasm_colon: (i32, i32),
    pub(super) gen_0: (i32, i32),
    pub(super) gen_1: (i32, i32),
    pub(super) gen_2: (i32, i32),
    pub(super) gen_3: (i32, i32),
    pub(super) ctx_res: (i32, i32),
    pub(super) pat_hdr: (i32, i32),
    pub(super) pat_body: (i32, i32),
    pub(super) passive_clean: (i32, i32),
    pub(super) passive_hit_hdr: (i32, i32),
    pub(super) passive_hit_body: (i32, i32),
}

fn encode_body(body: envelope::Body) -> Vec<u8> {
    Envelope {
        body: Some(body),
        ..Default::default()
    }
    .encode_bytes()
}

fn strip_msg_prefix(bytes: &[u8], tag_bytes: usize) -> Vec<u8> {
    let mut i = tag_bytes;
    while bytes[i] >= 0x80 {
        i += 1;
    }
    bytes[i + 1..].to_vec()
}

fn unwrap_env_body(body: envelope::Body) -> Vec<u8> {
    strip_msg_prefix(&encode_body(body), 2)
}

fn status_patch_op(text: &str) -> Vec<u8> {
    let replacement = panel::Node::new(
        3,
        "状态",
        node::Kind::Text(Text {
            text: text.into(),
            tone: panel::Tone::Foreground as i32,
        }),
    );
    unwrap_env_body(envelope::Body::Patch(panel::PanelPatch {
        base_revision: 0,
        revision: 0,
        operations: vec![panel::PatchOperation {
            node_id: 3,
            change: Some(panel::patch_operation::Change::Replace(replacement)),
        }],
    }))
}

fn req_hdr_field(name: &str, value: &[u8]) -> Vec<u8> {
    let inner = unwrap_env_body(envelope::Body::SessionActionResult(SessionActionResult {
        modified: false,
        request: Some(RequestDto {
            method: String::new(),
            url: String::new(),
            version: 0,
            headers: vec![HeaderDto {
                name: name.into(),
                value: value.to_vec(),
            }],
            body: vec![],
            trailers: vec![],
        }),
    }));
    strip_msg_prefix(&inner, 1)
}

fn resp_hdr_field(name: &str, value: &[u8]) -> Vec<u8> {
    let inner = unwrap_env_body(envelope::Body::HttpHookResult(HttpHookResult {
        action: 0,
        request: None,
        response: Some(ResponseDto {
            status: 0,
            version: 0,
            headers: vec![HeaderDto {
                name: name.into(),
                value: value.to_vec(),
            }],
            body: vec![],
            trailers: vec![],
        }),
    }));
    strip_msg_prefix(&inner, 1)
}

fn passive_hit_env(parameter: &str, evidence: &str) -> Vec<u8> {
    encode_body(envelope::Body::PassiveCheckResult(PassiveCheckResult {
        findings: vec![PluginFindingDto {
            title: "Wasm 被动合规检查命中敏感标记".into(),
            detail: format!("lab.wasm 在响应中检测到敏感标记 `{evidence}`。"),
            remediation: "移除响应中的 x-plugin-audit-flag 调试头与 PLUGIN_VULN_MARKER 标记。"
                .into(),
            severity: "High".into(),
            confidence: "Certain".into(),
            parameter: parameter.into(),
            evidence: evidence.into(),
        }],
        log_line: format!("lab.wasm passive_check: hit {evidence}"),
    }))
}

pub(super) fn build_consts() -> (Consts, Vec<u8>) {
    let mut blob = Vec::with_capacity(1536);
    let mut add = |bytes: &[u8]| -> (i32, i32) {
        while blob.len() & 3 != 0 {
            blob.push(0);
        }
        let ptr = CONST_BASE + blob.len() as i32;
        blob.extend_from_slice(bytes);
        (ptr, bytes.len() as i32)
    };

    let initial_snap = PanelSnapshot {
        revision: 1,
        title: "lab.wasm".into(),
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
                                    enabled: true,
                                    commit_editors: false,
                                }),
                            )],
                        }),
                    ),
                    panel::Node::new(
                        3,
                        "状态",
                        node::Kind::Text(Text {
                            text: "lab.wasm (wasm) 受限沙箱运行中".into(),
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
    };
    let n30_snap = unwrap_env_body(envelope::Body::Snapshot(PanelSnapshot {
        revision: 0,
        title: String::new(),
        root: Some(panel::Node {
            id: 30,
            accessibility_label: "消息载荷".into(),
            layout: None,
            kind: None,
        }),
    }));
    let gen_env = |items: &[&[u8]]| {
        encode_body(envelope::Body::PayloadResult(PayloadResult {
            outputs: items.iter().map(|s| s.to_vec()).collect(),
        }))
    };

    let consts = Consts {
        hello_snap: add(&encode_body(envelope::Body::Snapshot(initial_snap))),
        ack: add(&encode_body(envelope::Body::Ack(panel::Empty {}))),
        err_inv: add(&encode_body(envelope::Body::Error(Failure::new(
            ErrorCode::Invalid,
            "Wasm Envelope 校验失败或状态不匹配",
        )))),
        pref_call: add(&unwrap_env_body(envelope::Body::HostCall(HostCall {
            operation: Some(Operation::PreferencesSet(StorageEntry {
                key: "wasm_tag".into(),
                value: b"envelope-v13".to_vec(),
            })),
        }))),
        op_pref: add(&status_patch_op("已写入全局偏好: envelope-v13")),
        op_open: add(&status_patch_op("已加载消息载荷")),
        n30_hdr: add(&strip_msg_prefix(&n30_snap, 1)),
        hdr_req: add(&req_hdr_field("x-mio-wasm-hook", b"sandboxed")),
        hdr_resp: add(&resp_hdr_field("x-mio-wasm-resp", b"verified")),
        ws_pref: add(b"[wasm] "),
        hdr_sess: add(&req_hdr_field("x-mio-wasm-session", b"token-v1")),
        wasm_colon: add(b"wasm:"),
        gen_0: add(&gen_env(&[])),
        gen_1: add(&gen_env(&[b"wasm-gen-1"])),
        gen_2: add(&gen_env(&[b"wasm-gen-1", b"wasm-gen-2"])),
        gen_3: add(&gen_env(&[b"wasm-gen-1", b"wasm-gen-2", b"wasm-gen-3"])),
        ctx_res: add(&encode_body(envelope::Body::ContextMenuResult(
            ContextMenuResult {
                status_message: "Wasm 上下文菜单动作已完成".into(),
                log_line: "lab.wasm context_menu: executed".into(),
                open_repeater: None,
                replace_request: None,
            },
        ))),
        pat_hdr: add(b"x-plugin-audit-flag"),
        pat_body: add(b"plugin_vuln_marker"),
        passive_clean: add(&encode_body(envelope::Body::PassiveCheckResult(
            PassiveCheckResult {
                findings: vec![],
                log_line: String::new(),
            },
        ))),
        passive_hit_hdr: add(&passive_hit_env(
            "X-Plugin-Audit-Flag",
            "X-Plugin-Audit-Flag",
        )),
        passive_hit_body: add(&passive_hit_env("response_body", "PLUGIN_VULN_MARKER")),
    };
    assert!(
        (CONST_BASE as usize) + blob.len() <= HEAP_BASE as usize,
        "Wasm 静态常量区越过 HEAP_BASE"
    );
    (consts, blob)
}
