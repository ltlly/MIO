use std::{collections::HashMap, io};

use mio_plugin_sdk::{
    HostClient, HttpRequest, HttpResponse, MioPlugin, PanelUpdate, Uuid,
    panel::*,
    protocol::{self, *},
    run_stdio,
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
        Self::for_panel(1, vec![])
    }

    fn for_panel(revision: u64, grants: Vec<Permission>) -> Self {
        Self {
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

struct ProtocolLabPlugin {
    panels: HashMap<u64, Lab>,
}

impl ProtocolLabPlugin {
    fn new() -> Self {
        Self {
            panels: HashMap::from([(PANEL_ID, Lab::new())]),
        }
    }

    fn main_lab_mut(&mut self) -> Result<&mut Lab, String> {
        self.panels
            .get_mut(&PANEL_ID)
            .ok_or_else(|| "主面板未初始化".to_owned())
    }
}

impl MioPlugin for ProtocolLabPlugin {
    fn id(&self) -> &str {
        "lab.protocol"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn required_components(&self) -> Vec<Component> {
        vec![
            Component::Row,
            Component::Column,
            Component::Split,
            Component::Tabs,
            Component::Text,
            Component::Button,
            Component::Editor,
            Component::VirtualTable,
            Component::Canvas,
            Component::ScrollColumn,
            Component::Checkbox,
            Component::Select,
            Component::Badge,
            Component::Progress,
        ]
    }

    fn supports_panel_multiplexing(&self) -> bool {
        true
    }

    fn initial_panel(&self) -> PanelSnapshot {
        self.panels[&PANEL_ID].snapshot()
    }

    fn on_ready(&mut self, ctx: &mut dyn HostClient) -> Result<PanelUpdate, String> {
        let grants = ctx.grants().to_vec();
        let rev = ctx.next_panel_revision();
        let snapshot = {
            let lab = self.main_lab_mut()?;
            lab.grants = grants;
            lab.snapshot_at(rev)?
        };
        ctx.publish_panel(PanelUpdate::Snapshot(snapshot))?;
        if ctx.has_permission(Permission::TrafficRead) {
            self.main_lab_mut()?.query(ctx, 0, 64)?;
        }
        Ok(PanelUpdate::None)
    }

    fn on_ui_event(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        event: UiEvent,
    ) -> Result<PanelUpdate, String> {
        let lab = self
            .panels
            .get_mut(&panel_id)
            .ok_or_else(|| "面板已关闭".to_owned())?;
        lab.event(ctx, event)
    }

    fn on_open_request(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        open: OpenRequest,
    ) -> Result<PanelUpdate, String> {
        let grants = ctx.grants().to_vec();
        let lab = self
            .panels
            .entry(panel_id)
            .or_insert_with(|| Lab::for_panel(0, grants));
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
            Ok(()) => {
                let rev = ctx.next_panel_revision();
                Ok(PanelUpdate::Snapshot(lab.snapshot_at(rev)?))
            }
            Err(error) => {
                lab.status.clone_from(&error);
                lab.error = true;
                if panel_id == PANEL_ID || ctx.current_panel_revision() > 0 {
                    let rev = ctx.next_panel_revision();
                    if let Ok(snap) = lab.snapshot_at(rev) {
                        ctx.enqueue_panel_update(PanelUpdate::Snapshot(snap));
                    }
                }
                Err(error)
            }
        }
    }

    fn on_open_response(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        open: OpenResponse,
    ) -> Result<PanelUpdate, String> {
        let grants = ctx.grants().to_vec();
        let lab = self
            .panels
            .entry(panel_id)
            .or_insert_with(|| Lab::for_panel(0, grants));
        let result = (|| {
            lab.load_response(ResponseContext {
                resource: protocol::uuid(&open.resource_id)?,
                revision: open.message_revision,
                read_only: open.read_only,
                response: open.response.ok_or("缺少响应")?.try_into()?,
            })
        })();
        match result {
            Ok(()) => {
                let rev = ctx.next_panel_revision();
                Ok(PanelUpdate::Snapshot(lab.snapshot_at(rev)?))
            }
            Err(error) => {
                lab.status.clone_from(&error);
                lab.error = true;
                if panel_id == PANEL_ID || ctx.current_panel_revision() > 0 {
                    let rev = ctx.next_panel_revision();
                    if let Ok(snap) = lab.snapshot_at(rev) {
                        ctx.enqueue_panel_update(PanelUpdate::Snapshot(snap));
                    }
                }
                Err(error)
            }
        }
    }

    fn on_close_request(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        close: CloseRequest,
    ) -> Result<PanelUpdate, String> {
        let resource = protocol::uuid(&close.resource_id)?;
        let Some(lab) = self.panels.get_mut(&panel_id) else {
            return Ok(PanelUpdate::None);
        };
        if lab.close(ctx, resource)? {
            let rev = ctx.next_panel_revision();
            Ok(PanelUpdate::Snapshot(lab.snapshot_at(rev)?))
        } else {
            Ok(PanelUpdate::None)
        }
    }

    fn on_panel_destroy(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        close: CloseRequest,
    ) -> Result<PanelUpdate, String> {
        let resource = protocol::uuid(&close.resource_id)?;
        if let Some(mut lab) = self.panels.remove(&panel_id) {
            let _ = lab.close(ctx, resource)?;
        }
        Ok(PanelUpdate::None)
    }

    fn on_commit_editor(
        &mut self,
        _ctx: &mut dyn HostClient,
        panel_id: u64,
        commit: CommitEditor,
    ) -> Result<Committed, String> {
        let lab = self
            .panels
            .get_mut(&panel_id)
            .ok_or_else(|| "面板已关闭".to_owned())?;
        lab.commit(commit)
    }

    fn on_commit_response_editor(
        &mut self,
        _ctx: &mut dyn HostClient,
        panel_id: u64,
        commit: CommitResponseEditor,
    ) -> Result<ResponseCommitted, String> {
        let lab = self
            .panels
            .get_mut(&panel_id)
            .ok_or_else(|| "面板已关闭".to_owned())?;
        lab.commit_response(commit)
    }

    fn on_context_menu(
        &mut self,
        _ctx: &mut dyn HostClient,
        call: ContextMenuCall,
    ) -> Result<ContextMenuResult, String> {
        self.main_lab_mut()?.context_menu(call)
    }

    fn on_http_hook(
        &mut self,
        _ctx: &mut dyn HostClient,
        call: HttpHookCall,
    ) -> Result<HttpHookResult, String> {
        self.main_lab_mut()?.http_hook(call)
    }

    fn on_ws_hook(
        &mut self,
        _ctx: &mut dyn HostClient,
        call: WebSocketHookCall,
    ) -> Result<WebSocketHookResult, String> {
        Ok(self.main_lab_mut()?.ws_hook(call))
    }

    fn on_session_action(
        &mut self,
        _ctx: &mut dyn HostClient,
        call: SessionActionCall,
    ) -> Result<SessionActionResult, String> {
        self.main_lab_mut()?.session_action(call)
    }

    fn on_payload(
        &mut self,
        _ctx: &mut dyn HostClient,
        call: PayloadCall,
    ) -> Result<PayloadResult, String> {
        self.main_lab_mut()?.payload(call)
    }

    fn on_passive_check(
        &mut self,
        _ctx: &mut dyn HostClient,
        call: PassiveCheckCall,
    ) -> Result<PassiveCheckResult, String> {
        self.main_lab_mut()?.passive_check(call)
    }

    fn on_active_check(
        &mut self,
        _ctx: &mut dyn HostClient,
        call: ActiveCheckCall,
    ) -> Result<ActiveCheckResult, String> {
        self.main_lab_mut()?.active_check(call)
    }

    fn on_host_result(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        request_id: u64,
        result: HostResult,
    ) -> Result<PanelUpdate, String> {
        let Some(lab) = self.panels.get_mut(&panel_id) else {
            return Ok(PanelUpdate::None);
        };
        lab.host_result(ctx, request_id, result)
    }

    fn on_cancel(
        &mut self,
        _ctx: &mut dyn HostClient,
        panel_id: u64,
        _cancel: Cancel,
    ) -> Result<PanelUpdate, String> {
        if let Some(lab) = self.panels.get_mut(&panel_id) {
            lab.status = "操作已取消；不会自动重发。".into();
            lab.error = false;
        }
        Ok(PanelUpdate::None)
    }

    fn on_resync(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        snapshot: PanelSnapshot,
    ) -> Result<PanelUpdate, String> {
        let lab = self
            .panels
            .get_mut(&panel_id)
            .ok_or_else(|| "面板已关闭".to_owned())?;
        lab.resync(&snapshot)?;
        let rev = ctx.next_panel_revision();
        Ok(PanelUpdate::Snapshot(lab.snapshot_at(rev)?))
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
    run_stdio(ProtocolLabPlugin::new())
}
