use std::{collections::HashMap, io};

use mio_plugin_api::{
    HttpRequest, HttpResponse, Uuid,
    panel::*,
    protocol::{self, *},
};

const RAW_NODE: u64 = 30;
const DECODED_NODE: u64 = 31;
const RESPONSE_NODE: u64 = 32;
const RAW_DOCUMENT: u64 = 100;
const DECODED_DOCUMENT: u64 = 101;
const RESPONSE_DOCUMENT: u64 = 102;
const TABLE_NODE: u64 = 20;
const TABS_NODE: u64 = 22;
const CANVAS_NODE: u64 = 40;
const DECODE: u64 = 10;
const ENCODE: u64 = 11;
const SEND: u64 = 12;
const OPEN_REPEATER: u64 = 13;
const CANCEL: u64 = 14;
const WRITE_BACK: u64 = 15;
const REFRESH: u64 = 16;
const TOGGLE_HOOKS: u64 = 17;
const OPEN_INTRUDER: u64 = 18;
const SAVE_KV: u64 = 19;
const LOAD_KV: u64 = 21;
const SAVE_PREF: u64 = 23;
const LOAD_PREF: u64 = 24;
const AUTO_HOOK_CHECKBOX: u64 = 25;
const CODEC_SELECT: u64 = 26;
const STATUS_BADGE: u64 = 27;
const STATUS_PROGRESS: u64 = 28;
const STATUS_ROW: u64 = 29;

#[derive(Clone)]
struct Context {
    resource: Uuid,
    revision: u64,
    read_only: bool,
    is_draft: bool,
    request: HttpRequest,
}

#[derive(Clone)]
struct ResponseContext {
    resource: Uuid,
    revision: u64,
    read_only: bool,
    response: HttpResponse,
}

enum PendingService {
    History {
        viewport_revision: u64,
    },
    Detail {
        selection: u64,
    },
    Send {
        context_generation: u64,
    },
    OpenRepeater,
    OpenIntruder,
    WriteBack {
        request: HttpRequest,
        context_generation: u64,
    },
    SaveKv,
    LoadKv,
    SavePref,
    LoadPref,
}

struct Lab {
    panel_id: u64,
    revision: u64,
    raw: Editor,
    decoded: Editor,
    response: Option<Editor>,
    active_document: u64,
    split_fraction: f32,
    context: Option<Context>,
    response_context: Option<ResponseContext>,
    context_generation: u64,
    history: HistoryPage,
    viewport_revision: u64,
    selection_revision: u64,
    filter: String,
    positions: HashMap<String, Point>,
    hit_ids: HashMap<String, u64>,
    next_hit_id: u64,
    dragging: Option<(u64, Point)>,
    sending: bool,
    auto_tag_hooks: bool,
    url_safe_base64: bool,
    status: String,
    error: bool,
    grants: Vec<Permission>,
    pending: HashMap<u64, PendingService>,
}

fn empty_editor(document_id: u64, read_only: bool) -> Editor {
    Editor {
        document_id,
        revision: 1,
        contents: vec![],
        format: EditorFormat::Utf8 as i32,
        read_only,
    }
}

impl Lab {
    fn new() -> Self {
        Self::for_panel(PANEL_ID, 1, vec![])
    }

    fn for_panel(panel_id: u64, revision: u64, grants: Vec<Permission>) -> Self {
        Self {
            panel_id,
            revision,
            raw: empty_editor(RAW_DOCUMENT, false),
            decoded: empty_editor(DECODED_DOCUMENT, false),
            response: None,
            active_document: RAW_DOCUMENT,
            split_fraction: 0.35,
            context: None,
            response_context: None,
            context_generation: 0,
            history: HistoryPage {
                offset: 0,
                total: 0,
                rows: vec![],
            },
            viewport_revision: 0,
            selection_revision: 0,
            filter: String::new(),
            positions: HashMap::new(),
            hit_ids: HashMap::new(),
            next_hit_id: 1,
            dragging: None,
            sending: false,
            auto_tag_hooks: false,
            url_safe_base64: false,
            status: "选择项目流量，或在 Raw / Decoded 中输入内容进行 Base64 编解码。尚未发送请求。"
                .into(),
            error: false,
            grants,
            pending: HashMap::new(),
        }
    }
}

mod lab_hooks;
mod lab_panel;
mod lab_state;
mod lab_wires;
#[cfg(test)]
mod tests_hooks;
#[cfg(test)]
mod tests_state;

fn main() -> io::Result<()> {
    let mut main_lab = Lab::new();
    let registration = Register {
        plugin_id: "lab.protocol".into(),
        plugin_version: "0.1.0".into(),
        required_components: (0..=13).collect(),
        panel: Some(main_lab.snapshot()),
        supports_panel_multiplexing: true,
    };
    let mut session = WorkerSession::accept(io::stdin().lock(), io::stdout().lock(), registration)?;
    main_lab.grants.clone_from(&session.grants);
    main_lab.publish(&mut session, 0)?;
    if main_lab.grants.contains(&Permission::TrafficRead) {
        main_lab.query(&mut session, 0, 64)?;
    }
    let mut panels: HashMap<u64, Lab> = HashMap::from([(PANEL_ID, main_lab)]);
    loop {
        let envelope = session.receive()?;
        let reply = envelope.request_id;
        let panel_id = envelope.panel_id;
        match envelope.body.ok_or_else(|| io::Error::other("空消息"))? {
            envelope::Body::UiEvent(event) => {
                if let Some(lab) = panels.get_mut(&panel_id) {
                    lab.event(&mut session, event, reply)?;
                } else {
                    session.send_on_panel(
                        panel_id,
                        envelope::Body::Error(Failure::new(ErrorCode::Stale, "面板已关闭")),
                        0,
                        reply,
                    )?;
                }
            }
            envelope::Body::OpenRequest(open) => {
                let grants = session.grants.clone();
                let lab = panels
                    .entry(panel_id)
                    .or_insert_with(|| Lab::for_panel(panel_id, 0, grants));
                let result = (|| {
                    lab.load(Context {
                        resource: protocol::uuid(&open.resource_id)?,
                        revision: open.message_revision,
                        read_only: open.read_only,
                        is_draft: true,
                        request: open.request.ok_or("缺少请求")?.try_into()?,
                    })
                })();
                match result {
                    Ok(()) => lab.publish(&mut session, reply)?,
                    Err(error) => {
                        lab.status.clone_from(&error);
                        lab.error = true;
                        session.send_on_panel(
                            panel_id,
                            envelope::Body::Error(Failure::new(ErrorCode::Invalid, error)),
                            lab.revision,
                            reply,
                        )?;
                        if panel_id == PANEL_ID || lab.revision > 0 {
                            lab.publish(&mut session, 0)?;
                        }
                    }
                }
            }
            envelope::Body::OpenResponse(open) => {
                let grants = session.grants.clone();
                let lab = panels
                    .entry(panel_id)
                    .or_insert_with(|| Lab::for_panel(panel_id, 0, grants));
                let result = (|| {
                    lab.load_response(ResponseContext {
                        resource: protocol::uuid(&open.resource_id)?,
                        revision: open.message_revision,
                        read_only: open.read_only,
                        response: open.response.ok_or("缺少响应")?.try_into()?,
                    })
                })();
                match result {
                    Ok(()) => lab.publish(&mut session, reply)?,
                    Err(error) => {
                        lab.status.clone_from(&error);
                        lab.error = true;
                        session.send_on_panel(
                            panel_id,
                            envelope::Body::Error(Failure::new(ErrorCode::Invalid, error)),
                            lab.revision,
                            reply,
                        )?;
                        if panel_id == PANEL_ID || lab.revision > 0 {
                            lab.publish(&mut session, 0)?;
                        }
                    }
                }
            }
            envelope::Body::CommitEditor(commit) => {
                let Some(lab) = panels.get_mut(&panel_id) else {
                    session.send_on_panel(
                        panel_id,
                        envelope::Body::Error(Failure::new(ErrorCode::Stale, "面板已关闭")),
                        0,
                        reply,
                    )?;
                    continue;
                };
                let body = match lab.commit(commit) {
                    Ok(committed) => envelope::Body::Committed(committed),
                    Err(error) => envelope::Body::Error(Failure::new(ErrorCode::Stale, error)),
                };
                session.send_on_panel(panel_id, body, lab.revision, reply)?;
            }
            envelope::Body::CommitResponseEditor(commit) => {
                let Some(lab) = panels.get_mut(&panel_id) else {
                    session.send_on_panel(
                        panel_id,
                        envelope::Body::Error(Failure::new(ErrorCode::Stale, "面板已关闭")),
                        0,
                        reply,
                    )?;
                    continue;
                };
                let body = match lab.commit_response(commit) {
                    Ok(committed) => envelope::Body::ResponseCommitted(committed),
                    Err(error) => envelope::Body::Error(Failure::new(ErrorCode::Stale, error)),
                };
                session.send_on_panel(panel_id, body, lab.revision, reply)?;
            }
            envelope::Body::ContextMenuCall(call) => {
                let lab = panels.get_mut(&PANEL_ID).unwrap();
                let body = match lab.context_menu(call) {
                    Ok(result) => envelope::Body::ContextMenuResult(result),
                    Err(error) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, error)),
                };
                session.send_on_panel(PANEL_ID, body, lab.revision, reply)?;
            }
            envelope::Body::HttpHookCall(call) => {
                let lab = panels.get_mut(&PANEL_ID).unwrap();
                let body = match lab.http_hook(call) {
                    Ok(result) => envelope::Body::HttpHookResult(result),
                    Err(error) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, error)),
                };
                session.send_on_panel(PANEL_ID, body, lab.revision, reply)?;
            }
            envelope::Body::WebSocketHookCall(call) => {
                let lab = panels.get_mut(&PANEL_ID).unwrap();
                session.send_on_panel(
                    PANEL_ID,
                    envelope::Body::WebSocketHookResult(lab.ws_hook(call)),
                    lab.revision,
                    reply,
                )?;
            }
            envelope::Body::SessionActionCall(call) => {
                let lab = panels.get_mut(&PANEL_ID).unwrap();
                let body = match lab.session_action(call) {
                    Ok(result) => envelope::Body::SessionActionResult(result),
                    Err(error) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, error)),
                };
                session.send_on_panel(PANEL_ID, body, lab.revision, reply)?;
            }
            envelope::Body::PayloadCall(call) => {
                let lab = panels.get_mut(&PANEL_ID).unwrap();
                let body = match lab.payload(call) {
                    Ok(result) => envelope::Body::PayloadResult(result),
                    Err(error) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, error)),
                };
                session.send_on_panel(PANEL_ID, body, lab.revision, reply)?;
            }
            envelope::Body::PassiveCheckCall(call) => {
                let lab = panels.get_mut(&PANEL_ID).unwrap();
                let body = match lab.passive_check(call) {
                    Ok(result) => envelope::Body::PassiveCheckResult(result),
                    Err(error) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, error)),
                };
                session.send_on_panel(PANEL_ID, body, lab.revision, reply)?;
            }
            envelope::Body::ActiveCheckCall(call) => {
                let lab = panels.get_mut(&PANEL_ID).unwrap();
                let body = match lab.active_check(call) {
                    Ok(result) => envelope::Body::ActiveCheckResult(result),
                    Err(error) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, error)),
                };
                session.send_on_panel(PANEL_ID, body, lab.revision, reply)?;
            }
            envelope::Body::CloseRequest(close) => {
                let resource = protocol::uuid(&close.resource_id).map_err(io::Error::other)?;
                if close.destroy_panel && panel_id != PANEL_ID {
                    let rev = if let Some(mut lab) = panels.remove(&panel_id) {
                        let _ = lab.close(&mut session, resource)?;
                        lab.revision
                    } else {
                        0
                    };
                    session.send_on_panel(panel_id, envelope::Body::Ack(Empty {}), rev, reply)?;
                } else if let Some(lab) = panels.get_mut(&panel_id) {
                    if lab.close(&mut session, resource)? {
                        lab.publish(&mut session, reply)?;
                    } else {
                        session.send_on_panel(
                            panel_id,
                            envelope::Body::Ack(Empty {}),
                            lab.revision,
                            reply,
                        )?;
                    }
                } else {
                    session.send_on_panel(panel_id, envelope::Body::Ack(Empty {}), 0, reply)?;
                }
            }
            envelope::Body::HostResult(result) => {
                if let Some(lab) = panels.get_mut(&panel_id) {
                    lab.host_result(&mut session, envelope.in_reply_to, result)?;
                }
            }
            envelope::Body::Cancel(_) => {
                if let Some(lab) = panels.get_mut(&panel_id) {
                    lab.status = "操作已取消；不会自动重发。".into();
                    lab.error = false;
                    lab.publish(&mut session, reply)?;
                } else {
                    session.send_on_panel(panel_id, envelope::Body::Ack(Empty {}), 0, reply)?;
                }
            }
            envelope::Body::Resync(snapshot) => {
                if let Some(lab) = panels.get_mut(&panel_id) {
                    lab.resync(&snapshot)?;
                    lab.publish(&mut session, reply)?;
                } else {
                    session.send_on_panel(
                        panel_id,
                        envelope::Body::Error(Failure::new(ErrorCode::Stale, "面板已关闭")),
                        0,
                        reply,
                    )?;
                }
            }
            envelope::Body::Shutdown(_) => break,
            _ => return Err(io::Error::other("worker 收到方向无效的消息")),
        }
    }
    Ok(())
}
