use std::collections::VecDeque;

use mio_core::{
    HttpRequest, IntruderTab, LoadedPlugin, PluginInspection, PluginWorkspaceRequest, Uuid,
    panel::EditorSnapshot, protocol::Permission,
};

use crate::plugin_panel::NativePanel;

use super::*;

mod script_library;
mod context_menu_extract;
mod context_viewers;
mod custom_scan_checks;
mod inspect_load;
mod lifecycle;
mod operations;
mod payload_hooks;
mod render_messages;
mod render_response;
mod render_suite;
mod repeater_editor;
mod response_viewers;
#[cfg(test)]
mod tests;
mod workspace_ops;

const MAX_PLUGIN_LOGS: usize = 200;
const MAX_PLUGIN_LOG_CHARS: usize = 512;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum ExtensionsPage {
    #[default]
    Installed,
    PluginStore,
    Apis,
    CustomScanChecks,
    ScriptPresetLibrary,
    Settings,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum ExtensionsDetailTab {
    #[default]
    Details,
    Output,
    Errors,
}

pub(super) struct Extensions {
    directory: Entity<InputState>,
    search: Entity<InputState>,
    search_subscription: Option<Subscription>,
    page: ExtensionsPage,
    detail_tab: ExtensionsDetailTab,
    add_panel_open: bool,
    auto_reload: HashSet<Uuid>,
    inspection: Option<Inspection>,
    instances: Vec<Instance>,
    selected: Option<Uuid>,
    pub(in crate::workbench) active_top_suite_tab: Option<Uuid>,
    operations: HashMap<Uuid, Operation>,
    create_after_save: Vec<CreateEditor>,
    pub(super) active_messages: HashMap<TabId, Uuid>,
    pub(super) active_responses: HashMap<TabId, Uuid>,
    context_viewers: HashMap<&'static str, ContextViewer>,
    response_viewers: HashMap<String, ResponseViewer>,
    saved_commits: HashMap<TabId, SavedCommit>,
    workspace_requests: VecDeque<PluginWorkspaceRequest>,
    logs: VecDeque<String>,
    error_logs: VecDeque<String>,
    output_selected_all: bool,
    errors_selected_all: bool,
    script_library: script_library::ScriptPresetLibraryUi,
    custom_checks: custom_scan_checks::CustomScanChecksUi,
    allow_quit: bool,
}

struct ContextViewer {
    context_id: Uuid,
    revision: u64,
    read_only: bool,
    request: HttpRequest,
    instance_id: Option<Uuid>,
    committing: bool,
}

struct ResponseViewer {
    context_id: Uuid,
    revision: u64,
    read_only: bool,
    response: HttpResponse,
    instance_id: Option<Uuid>,
    committing: bool,
}

struct Inspection {
    info: PluginInspection,
    trusted: bool,
    grants: HashSet<Permission>,
    replace: Option<Uuid>,
}

struct Instance {
    info: LoadedPlugin,
    panel: Entity<NativePanel>,
    live: bool,
    status: String,
    bound: Option<Draft>,
    document: Option<u64>,
    context_owner: Option<&'static str>,
    _subscription: Subscription,
}

#[derive(Clone)]
pub(in crate::workbench) struct CreateEditor {
    source: Uuid,
    tab_id: TabId,
    draft: Draft,
    replace: Option<Uuid>,
}

struct SavedCommit {
    instance: Uuid,
    editor: EditorSnapshot,
    draft: Draft,
}

pub(in crate::workbench) enum WorkspaceMutation {
    Repeater {
        before: Option<WorkTab>,
        proposed: WorkTab,
        active_before: Option<TabId>,
    },
    Intruder {
        proposed: IntruderTab,
        active_before: Option<TabId>,
        orientation_before: Orientation,
    },
}

pub(in crate::workbench) enum Operation {
    Inspect {
        replace: Option<Uuid>,
    },
    Load {
        replace: Option<Uuid>,
    },
    Create(CreateEditor),
    Commit {
        instance: Uuid,
        tab_id: TabId,
        send: PendingSend,
        editor: EditorSnapshot,
    },
    Rebind {
        instance: Uuid,
        tab_id: TabId,
        draft: Draft,
    },
    Disable,
    Event {
        instance: Uuid,
        node_id: u64,
    },
    Reject,
    Cancel,
    Workspace {
        instance: Uuid,
        tab_id: TabId,
        revision: u64,
        mutation: Box<WorkspaceMutation>,
    },
    Reorder,
    OpenContext {
        owner: &'static str,
        context_id: Uuid,
        revision: u64,
    },
    CommitContext {
        owner: &'static str,
        instance: Uuid,
        context_id: Uuid,
        revision: u64,
        editor: EditorSnapshot,
    },
    OpenResponse {
        owner: String,
        tab_id: Option<TabId>,
        context_id: Uuid,
        revision: u64,
    },
    CommitResponse {
        owner: String,
        instance: Uuid,
        context_id: Uuid,
        revision: u64,
        editor: EditorSnapshot,
        forward_drop: Option<bool>,
    },
    GeneratePayloads {
        tab_id: TabId,
    },
    ProcessPayloads {
        tab_id: TabId,
        position: Option<usize>,
    },
    Reload {
        instance: Uuid,
    },
    SetAutoReload {
        instance: Uuid,
        enabled: bool,
    },
    ContextMenu {
        instance: Uuid,
    },
}

impl Extensions {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            directory: cx.new(|cx| {
                InputState::new(window, cx).placeholder("包含 manifest.json 的本地插件包目录")
            }),
            search: cx.new(|cx| InputState::new(window, cx).placeholder("搜索")),
            search_subscription: None,
            page: ExtensionsPage::Installed,
            detail_tab: ExtensionsDetailTab::Details,
            add_panel_open: false,
            auto_reload: HashSet::new(),
            inspection: None,
            instances: vec![],
            selected: None,
            active_top_suite_tab: None,
            operations: HashMap::new(),
            create_after_save: vec![],
            active_messages: HashMap::new(),
            active_responses: HashMap::new(),
            context_viewers: HashMap::new(),
            response_viewers: HashMap::new(),
            saved_commits: HashMap::new(),
            workspace_requests: VecDeque::new(),
            logs: VecDeque::new(),
            error_logs: VecDeque::new(),
            output_selected_all: false,
            errors_selected_all: false,
            script_library: script_library::ScriptPresetLibraryUi::new(window, cx),
            custom_checks: custom_scan_checks::CustomScanChecksUi::new(window, cx),
            allow_quit: false,
        }
    }

    fn sanitize_log_line(line: &str) -> Option<String> {
        let mut sanitized = String::new();
        let mut chars = line.chars().enumerate().peekable();
        for (_, ch) in chars.by_ref() {
            if sanitized.chars().count() >= MAX_PLUGIN_LOG_CHARS {
                sanitized.push('…');
                break;
            }
            sanitized.push(if ch.is_control() { ' ' } else { ch });
        }
        let trimmed = sanitized.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    }

    fn push_log(&mut self, line: impl AsRef<str>) {
        let Some(entry) = Self::sanitize_log_line(line.as_ref()) else {
            return;
        };
        if self.logs.len() >= MAX_PLUGIN_LOGS {
            self.logs.pop_front();
        }
        self.logs.push_back(entry);
    }

    fn push_error_log(&mut self, line: impl AsRef<str>) {
        let Some(entry) = Self::sanitize_log_line(line.as_ref()) else {
            return;
        };
        if self.error_logs.len() >= MAX_PLUGIN_LOGS {
            self.error_logs.pop_front();
        }
        self.error_logs.push_back(entry);
    }

    fn instance(&self, id: Uuid) -> Option<&Instance> {
        self.instances
            .iter()
            .find(|instance| instance.info.identity.instance_id == id)
    }

    fn instance_mut(&mut self, id: Uuid) -> Option<&mut Instance> {
        self.instances
            .iter_mut()
            .find(|instance| instance.info.identity.instance_id == id)
    }

    pub(in crate::workbench) fn is_live_suite_tab(&self, id: Uuid) -> bool {
        self.instance(id).is_some_and(Self::has_top_suite_tab)
    }

    pub(in crate::workbench) fn context_request_panel(
        &self,
        owner: &str,
    ) -> Option<&Entity<NativePanel>> {
        self.context_viewers
            .get(owner)
            .and_then(|v| v.instance_id)
            .and_then(|id| self.instance(id))
            .filter(|inst| inst.live)
            .map(|inst| &inst.panel)
    }

    pub(in crate::workbench) fn context_response_panel(
        &self,
        owner: &str,
    ) -> Option<&Entity<NativePanel>> {
        self.response_viewers
            .get(owner)
            .and_then(|v| v.instance_id)
            .and_then(|id| self.instance(id))
            .filter(|inst| inst.live)
            .map(|inst| &inst.panel)
    }

    pub(in crate::workbench) fn repeater_request_panel(
        &self,
        tab_id: TabId,
    ) -> Option<&Entity<NativePanel>> {
        self.active_messages
            .get(&tab_id)
            .copied()
            .and_then(|id| self.instance(id))
            .filter(|inst| inst.live)
            .map(|inst| &inst.panel)
    }

    pub(in crate::workbench) fn repeater_response_panel(
        &self,
        tab_id: TabId,
    ) -> Option<&Entity<NativePanel>> {
        let resp_owner = format!("repeater:{tab_id}");
        self.active_responses
            .get(&tab_id)
            .copied()
            .or_else(|| {
                self.response_viewers
                    .get(&resp_owner)
                    .and_then(|v| v.instance_id)
            })
            .and_then(|id| self.instance(id))
            .filter(|inst| inst.live)
            .map(|inst| &inst.panel)
    }

    fn has_top_suite_tab(instance: &Instance) -> bool {
        instance.live
            && instance.info.tab_id.is_none()
            && instance.context_owner.is_none()
            && instance
                .info
                .manifest
                .contributions
                .iter()
                .any(|c| c == "suite_panel")
    }

    pub(in crate::workbench) fn top_suite_tabs(&self, cx: &App) -> Vec<(Uuid, SharedString)> {
        self.instances
            .iter()
            .filter(|instance| Self::has_top_suite_tab(instance))
            .map(|instance| {
                let title = instance.panel.read(cx).title().trim();
                let label = if title.is_empty() {
                    instance.info.manifest.id.clone()
                } else {
                    title.to_owned()
                };
                (
                    instance.info.identity.instance_id,
                    SharedString::from(label),
                )
            })
            .collect()
    }
}

impl Workbench {
    pub(in crate::workbench) fn sync_active_top_suite_tab(&mut self) {
        if let Some(id) = self.extensions.active_top_suite_tab
            && (self.tool != Tool::Extensions || !self.extensions.is_live_suite_tab(id))
        {
            self.extensions.active_top_suite_tab = None;
        }
    }
}
