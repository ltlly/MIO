use base64::{Engine, engine::general_purpose::STANDARD};

use super::*;

pub(super) fn package() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    let bin = std::env::var("MIO_TEST_PROTOCOL_LAB_BIN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_protocol-lab").into());
    let entry = if cfg!(windows) {
        "protocol-lab.exe"
    } else {
        "protocol-lab"
    };
    fs::copy(&bin, directory.path().join(entry)).unwrap();
    fs::write(directory.path().join("manifest.json"), serde_json::to_vec(&serde_json::json!({
        "id":"lab.protocol", "version":"0.1.0", "api_version":1, "engine":"native", "entry":entry,
        "permissions":["traffic.read","draft.write","repeater.open","network.send"],
        "contributions":["suite_panel","message_editor"]
    })).unwrap()).unwrap();
    directory
}

pub(super) struct App {
    pub(super) runtime: Runtime,
    pub(super) client: Client,
    pub(super) project: ProjectKey,
    pub(super) workspace: Workspace,
    pub(super) panels: HashMap<Uuid, Arc<PanelSnapshot>>,
    pub(super) stopped: Vec<Uuid>,
}

impl App {
    pub(super) async fn start() -> Self {
        let (runtime, mut client) = Runtime::start().unwrap();
        let Event::ProjectOpened {
            project, workspace, ..
        } = timeout(BOUND, client.events.recv()).await.unwrap().unwrap()
        else {
            panic!("project opened");
        };
        Self {
            runtime,
            client,
            project,
            workspace,
            panels: HashMap::new(),
            stopped: vec![],
        }
    }

    pub(super) async fn next(&mut self) -> Event {
        let event = timeout(BOUND, self.client.events.recv())
            .await
            .expect("bounded product event")
            .expect("service connected");
        match &event {
            Event::PluginPanel {
                identity, snapshot, ..
            } => {
                self.panels.insert(identity.instance_id, snapshot.clone());
            }
            Event::WorkspaceChanged { workspace, .. } => self.workspace = workspace.clone(),
            Event::PluginStopped { identity, .. } => self.stopped.push(identity.instance_id),
            Event::Error { .. } => panic!("unexpected product failure: {event:?}"),
            _ => {}
        }
        event
    }

    pub(super) async fn action(&self, action: PluginAction) -> Uuid {
        let id = Uuid::new_v4();
        self.client
            .commands
            .send(Command::Plugin {
                project: self.project,
                operation_id: id,
                action,
            })
            .await
            .unwrap();
        id
    }

    pub(super) async fn outcome(&mut self, id: Uuid) -> Result<PluginOutcome, String> {
        timeout(BOUND, async {
            loop {
                if let Event::PluginOperation {
                    operation_id,
                    result,
                    ..
                } = self.next().await
                    && operation_id == id
                {
                    if let Ok(PluginOutcome::Loaded { info, panel }) = &result {
                        let cached = self
                            .panels
                            .entry(info.identity.instance_id)
                            .or_insert_with(|| panel.clone());
                        if panel.revision > cached.revision {
                            *cached = panel.clone();
                        }
                    }
                    return result;
                }
            }
        })
        .await
        .expect("bounded operation completion")
    }

    pub(super) async fn panel(
        &mut self,
        id: Uuid,
        ready: impl Fn(&PanelSnapshot) -> bool,
    ) -> Arc<PanelSnapshot> {
        timeout(BOUND, async {
            loop {
                if let Some(panel) = self.panels.get(&id)
                    && ready(panel)
                {
                    return panel.clone();
                }
                self.next().await;
            }
        })
        .await
        .expect("causal panel update")
    }

    pub(super) async fn load(
        &mut self,
        directory: &TempDir,
        replace: Option<Uuid>,
    ) -> LoadedPlugin {
        let op = self
            .action(PluginAction::Inspect {
                directory: directory.path().to_owned(),
            })
            .await;
        let PluginOutcome::Inspected(inspection) = self.outcome(op).await.unwrap() else {
            panic!("inspection");
        };
        if replace.is_none() {
            let denied = self
                .action(PluginAction::Load {
                    inspection_id: inspection.id,
                    native_trusted: false,
                    grants: vec![],
                    replace_instance: None,
                })
                .await;
            assert!(self.outcome(denied).await.unwrap_err().contains("OS 沙箱"));
        }
        let op = self
            .action(PluginAction::Load {
                inspection_id: inspection.id,
                native_trusted: true,
                grants: vec![
                    Permission::TrafficRead,
                    Permission::DraftWrite,
                    Permission::RepeaterOpen,
                    Permission::NetworkSend,
                ],
                replace_instance: replace,
            })
            .await;
        let PluginOutcome::Loaded { info, .. } = self.outcome(op).await.unwrap() else {
            panic!("load");
        };
        // The suite starts one History query after Ready; wait for that result before clicking.
        self.panel(info.identity.instance_id, |panel| {
            matches!(panel.root.as_ref().and_then(|root| root.find(4)).and_then(|node| node.kind.as_ref()),
                Some(node::Kind::Text(text)) if text.text.contains("条实际流量"))
        })
        .await;
        info
    }

    pub(super) async fn add_tab(&mut self, url: &str, contents: &str) -> Uuid {
        self.client
            .commands
            .send(Command::NewRepeater {
                project: self.project,
            })
            .await
            .unwrap();
        let mut tab = loop {
            if let Event::RepeaterOpened { tab, .. } = self.next().await {
                break tab;
            }
        };
        tab.draft.target = url.into();
        tab.draft.raw = format!(
            "POST /plugin HTTP/1.1\r\nX-Duplicate: one\r\nX-Duplicate: two\r\n\r\n{}",
            STANDARD.encode(contents)
        )
        .into_bytes();
        tab.draft.revision += 1;
        let id = tab.id;
        let mut workspace = self.workspace.clone();
        workspace.tabs.push(tab);
        workspace.active = Some(id);
        self.save(workspace).await;
        id
    }

    pub(super) async fn save(&mut self, mut workspace: Workspace) {
        workspace.revision += 1;
        let revision = workspace.revision;
        self.client
            .commands
            .send(Command::SaveWorkspace {
                project: self.project,
                workspace,
            })
            .await
            .unwrap();
        loop {
            match self.next().await {
                Event::WorkspaceChanged { workspace, .. } if workspace.revision == revision => {
                    break;
                }
                Event::WorkspaceRejected { message, .. } => panic!("workspace save: {message}"),
                _ => {}
            }
        }
    }

    pub(super) async fn open(&mut self, instance_id: Uuid, tab_id: Uuid) -> Arc<PanelSnapshot> {
        let tab = self
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .unwrap();
        let expected =
            mio_core::parse_request(&tab.draft.raw, &tab.draft.target, tab.draft.version)
                .unwrap()
                .body;
        let op = self
            .action(PluginAction::OpenRequest {
                instance_id,
                tab_id,
                draft_revision: tab.draft.revision,
            })
            .await;
        assert!(matches!(
            self.outcome(op).await.unwrap(),
            PluginOutcome::Opened { .. }
        ));
        self.panel(instance_id, |panel| editor(panel, 30).contents == expected)
            .await
    }

    pub(super) async fn click(
        &mut self,
        instance_id: Uuid,
        panel: &PanelSnapshot,
        node_id: u64,
        editors: Vec<EditorSnapshot>,
    ) {
        let op = self
            .action(PluginAction::Ui {
                instance_id,
                panel_revision: panel.revision,
                event_seq: 0,
                event: UiEvent {
                    node_id,
                    event: Some(ui_event::Event::Click(panel::Empty {})),
                    editors,
                },
            })
            .await;
        assert!(matches!(
            self.outcome(op).await.unwrap(),
            PluginOutcome::EventQueued
        ));
    }

    pub(super) fn shutdown(self) {
        self.runtime.shutdown().unwrap();
    }
}

pub(super) fn editor(panel: &PanelSnapshot, node_id: u64) -> &panel::Editor {
    let Some(node::Kind::Editor(editor)) = panel
        .root
        .as_ref()
        .unwrap()
        .find(node_id)
        .and_then(|node| node.kind.as_ref())
    else {
        panic!("editor");
    };
    editor
}

pub(super) fn snapshots(panel: &PanelSnapshot) -> Vec<EditorSnapshot> {
    let mut editors = Vec::new();
    panel.root.as_ref().unwrap().editors(&mut editors);
    editors
        .into_iter()
        .map(|(_, editor)| EditorSnapshot {
            document_id: editor.document_id,
            revision: editor.revision,
            contents: editor.contents.clone(),
            selection_start: 0,
            selection_end: 0,
        })
        .collect()
}
