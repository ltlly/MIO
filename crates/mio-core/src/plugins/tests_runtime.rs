use super::*;

pub(super) fn request() -> HttpRequest {
    HttpRequest {
        method: "GET".into(),
        url: "http://127.0.0.1/".into(),
        version: mio_plugin_api::HttpVersion::Http11,
        headers: vec![],
        body: vec![],
        trailers: vec![],
    }
}

pub(super) fn identity() -> ChannelIdentity {
    ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
    }
}

pub(super) fn running() -> (
    Running,
    mpsc::Receiver<ServiceRequest>,
    mpsc::Receiver<PluginEvent>,
) {
    let identity = identity();
    let (services, calls) = mpsc::channel(QUEUE_CAPACITY);
    let (events, updates) = mpsc::channel(QUEUE_CAPACITY);
    let panel = PanelState::new(PanelSnapshot {
        revision: 1,
        title: "test".into(),
        root: Some(panel::Node::new(
            1,
            "retained editor",
            panel::node::Kind::Editor(panel::Editor {
                document_id: 1,
                revision: 1,
                contents: b"retained".to_vec(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    })
    .unwrap();
    (
        Running {
            identity,
            plugin_id: "lab.fixture".into(),
            panel,
            broker: PermissionBroker::new(
                identity,
                [
                    Permission::TrafficRead,
                    Permission::DraftWrite,
                    Permission::RepeaterOpen,
                ],
            ),
            services,
            events,
            cancel: CancellationToken::new(),
            outgoing_sequence: 0,
            incoming_sequence: 0,
            active_resource: None,
            opening_request: None,
            rate_epoch: Instant::now(),
            frame_count: 0,
            requests: HashMap::new(),
            service_cancels: HashMap::new(),
            revoked_resources: HashMap::new(),
            service_tasks: JoinSet::new(),
        },
        calls,
        updates,
    )
}
