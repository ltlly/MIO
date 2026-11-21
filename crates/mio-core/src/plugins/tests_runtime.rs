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
            protocol_minor: protocol::PROTOCOL_MINOR,
            supports_panel_multiplexing: false,
            panel,
            panels: HashMap::new(),
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
            cancelled_requests: std::collections::VecDeque::new(),
            cancel_notifications: std::collections::VecDeque::new(),
            v14_degraded_resync_ids: std::collections::VecDeque::new(),
            main_cancelled_state_requests: std::collections::VecDeque::new(),
            main_cancelled_resync_requests: std::collections::VecDeque::new(),
            main_resync_incompatible: false,
            main_deferred_commands: std::collections::VecDeque::new(),
            outbound_queue: std::collections::VecDeque::new(),
            outbound_claims: None,
            pending_revokes: std::collections::VecDeque::new(),
            main_resync_needed_seq: 0,
            main_resync_attempts: 0,
            main_resync_semantic_drift: false,
            main_resync_snapshot_confirmed: false,
            main_last_snapshot_rev: 0,
            main_pending_pointer_cancels: std::collections::VecDeque::new(),
            next_resync_seq: 0,
            main_processed_ui_seq: 0,
            pending_panel_syncs: std::collections::BTreeMap::new(),
            defer_panel_sync_for_publish: false,
            child_headroom_blocked: false,
            urgent_commands: None,
            panel_destroy_control_seq: HashMap::new(),
            panel_destroy_seq_floor: 0,
            resource_revoke_control_seq: HashMap::new(),
            resource_revoke_seq_floor: 0,
            cancelled_op_control_seq: HashMap::new(),
            cancelled_op_seq_floor: 0,
            panel_resource_open_seq: HashMap::new(),
            panel_revoke_snapshots: HashMap::new(),
            panel_revoke_snapshot_floor: HashMap::new(),
            service_cancels: HashMap::new(),
            revoked_resources: HashMap::new(),
            panel_revoked_resources: HashMap::new(),
            service_tasks: JoinSet::new(),
        },
        calls,
        updates,
    )
}
