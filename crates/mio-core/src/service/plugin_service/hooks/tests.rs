use super::*;
use std::{collections::BTreeSet, fs, path::Path};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(windows)]
const NATIVE_MAGIC: [u8; 4] = [b'M', b'Z', 0, 0];
#[cfg(not(windows))]
const NATIVE_MAGIC: [u8; 4] = [0xcf, 0xfa, 0xed, 0xfe];

struct BlockingReader(tokio::io::DuplexStream, tokio::runtime::Handle);

impl std::io::Read for BlockingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use tokio::io::AsyncReadExt;
        self.1.block_on(self.0.read(buf))
    }
}

struct BlockingWriter(tokio::io::DuplexStream, tokio::runtime::Handle);

impl std::io::Write for BlockingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use tokio::io::AsyncWriteExt;
        self.1.block_on(self.0.write(buf))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        self.1.block_on(self.0.flush())
    }
}

struct HookTestPlugin;

impl mio_plugin_sdk::MioPlugin for HookTestPlugin {
    fn id(&self) -> &str {
        "lab.hooks"
    }

    fn version(&self) -> &str {
        "1"
    }

    fn on_http_hook(
        &mut self,
        _ctx: &mut dyn mio_plugin_sdk::HostClient,
        call: mio_plugin_sdk::protocol::HttpHookCall,
    ) -> Result<mio_plugin_sdk::protocol::HttpHookResult, String> {
        use mio_plugin_sdk::protocol::{HeaderDto, HookAction, HttpHookResult};
        if !call.response_stage {
            let mut req = call.request.ok_or("missing request")?;
            req.headers.push(HeaderDto {
                name: "x-mio-plugin-hook".into(),
                value: b"hooked".to_vec(),
            });
            return Ok(HttpHookResult {
                action: HookAction::Modify as i32,
                request: Some(req),
                response: None,
            });
        }
        Ok(HttpHookResult {
            action: HookAction::Pass as i32,
            request: None,
            response: None,
        })
    }

    fn on_ws_hook(
        &mut self,
        _ctx: &mut dyn mio_plugin_sdk::HostClient,
        call: mio_plugin_sdk::protocol::WebSocketHookCall,
    ) -> Result<mio_plugin_sdk::protocol::WebSocketHookResult, String> {
        use mio_plugin_sdk::protocol::{HookAction, WebSocketHookResult};
        let mut payload = call.payload;
        payload.extend_from_slice(b":ws");
        Ok(WebSocketHookResult {
            action: HookAction::Modify as i32,
            payload,
            ..Default::default()
        })
    }

    fn on_session_action(
        &mut self,
        _ctx: &mut dyn mio_plugin_sdk::HostClient,
        call: mio_plugin_sdk::protocol::SessionActionCall,
    ) -> Result<mio_plugin_sdk::protocol::SessionActionResult, String> {
        use mio_plugin_sdk::protocol::{HeaderDto, SessionActionResult};
        let mut req = call.request.ok_or("missing request")?;
        req.headers.push(HeaderDto {
            name: "x-mio-plugin-session".into(),
            value: b"session".to_vec(),
        });
        Ok(SessionActionResult {
            modified: true,
            request: Some(req),
        })
    }
}

fn request() -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        url: "http://127.0.0.1/plugin-hook".into(),
        version: HttpVersion::Http11,
        headers: vec![Header {
            name: "Content-Type".into(),
            value: b"application/octet-stream".to_vec(),
        }],
        body: vec![0, 0xff, b'x'],
        trailers: Vec::new(),
    }
}

fn write_native_hook_manifest(dir: &Path, grants: &[&str], contributions: &[&str]) {
    let entry = dir.join("hook-worker");
    fs::write(&entry, NATIVE_MAGIC).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o755)).unwrap();
    let manifest = serde_json::json!({
        "id": "lab.hooks",
        "version": "1",
        "api_version": 1,
        "engine": "native",
        "entry": "hook-worker",
        "permissions": grants,
        "contributions": contributions,
    });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

async fn spawn_hook_worker(
    dir: &Path,
    project: ProjectKey,
    grant_strs: &[&str],
    grants: Vec<Permission>,
    contributions: &[&str],
) -> (
    VerifiedPackage,
    NativeTrust,
    PluginSupervisor,
    mpsc::Receiver<ServiceRequest>,
    std::thread::JoinHandle<()>,
) {
    write_native_hook_manifest(dir, grant_strs, contributions);
    let package = VerifiedPackage::read(dir).unwrap();
    let trust = NativeTrust::confirmed(&package, grants.clone()).unwrap();
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project,
    };
    let (host_tx, worker_rx) = tokio::io::duplex(64 * 1024);
    let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
    let rt_handle = tokio::runtime::Handle::current();
    let worker_thread = std::thread::spawn(move || {
        let mut plugin = HookTestPlugin;
        let _ = mio_plugin_sdk::run_with_io(
            BlockingReader(worker_rx, rt_handle.clone()),
            BlockingWriter(worker_tx, rt_handle),
            &mut plugin,
        );
    });
    let grants_set: BTreeSet<Permission> = grants.into_iter().collect();
    let (worker, service_rx) = crate::plugins::spawn_test_duplex_supervisor(
        identity,
        package.manifest.clone(),
        grants_set,
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    (package, trust, worker, service_rx, worker_thread)
}

#[test]
fn depth_guard_uses_maximum_and_blocks_malformed_values() {
    let mut req = request();
    assert_eq!(plugin_hook_depth(&req), 0);
    req.headers.extend([
        Header {
            name: "x-mio-plugin-depth".into(),
            value: b"0".to_vec(),
        },
        Header {
            name: "X-MIO-PLUGIN-DEPTH".into(),
            value: b" 2 ".to_vec(),
        },
    ]);
    assert_eq!(plugin_hook_depth(&req), 2);
    for bad in [&b"256"[..], &b"not-a-depth"[..], &[0xff][..]] {
        req.headers[2].value = bad.to_vec();
        assert_eq!(plugin_hook_depth(&req), u8::MAX);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_only_plugins_trigger_intruder_pre_send_without_session_permission() {
    let root = tempfile::tempdir().unwrap();
    let (events, _rx) = mpsc::channel(128);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let (package, _trust, worker, service_rx, worker_thread) = spawn_hook_worker(
        root.path(),
        state.project,
        &["traffic.modify"],
        vec![Permission::TrafficModify],
        &["http_hook"],
    )
    .await;
    let identity = worker.identity;
    let panel = worker.initial_panel.clone();
    let manifest_modified = package.manifest_modified;
    let manifest_size = package.manifest_bytes.len() as u64;
    let entry_modified = package.modified;
    let entry_size = package.executable_size;
    let info = LoadedPlugin {
        identity,
        directory: package.directory.clone(),
        manifest: package.manifest.clone(),
        grants: vec![Permission::TrafficModify],
        tab_id: None,
    };
    state.plugins.instances.insert(
        identity.instance_id,
        Instance {
            worker,
            services: service_rx,
            package,
            manifest_modified,
            manifest_size,
            entry_modified,
            entry_size,
            auto_reload: false,
            hook_filter: PluginHookFilter::default(),
            hook_counters: std::sync::Arc::new(PluginHookCounters::default()),
            info,
            panel,
            resources: HashMap::new(),
        },
    );
    state.plugins.order.push(identity.instance_id);
    assert_eq!(state.session_action_hooks().len(), 1);
    assert!(state.strict_session_action_hooks().is_empty());
    assert_eq!(state.tool_http_hooks().len(), 1);
    let mut req = request();
    let outcome = run_pre_send_plugin_hooks(
        &state.strict_session_action_hooks(),
        &state.tool_http_hooks(),
        Uuid::new_v4(),
        protocol::SourceDto::Intruder,
        &mut req,
        Instant::now() + Duration::from_secs(3),
        &CancellationToken::new(),
    )
    .await;
    assert!(matches!(
        outcome,
        PreSendHookOutcome::Proceed { mutated: true }
    ));
    assert!(
        req.headers
            .iter()
            .any(|h| { h.name == "x-mio-plugin-hook" && h.value == b"hooked" })
    );
    assert!(!req.headers.iter().any(|h| h.name == "x-mio-plugin-session"));
    assert_eq!(req.body, [0, 0xff, b'x']);
    state.stop_plugins().await.unwrap();
    worker_thread.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_and_http_hooks_run_in_order_for_repeater_and_depth_skip_is_nondestructive() {
    let root = tempfile::tempdir().unwrap();
    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: 1,
    };
    let (_package, _trust, mut worker, _service_rx, worker_thread) = spawn_hook_worker(
        root.path(),
        project,
        &["traffic.modify", "session.hook"],
        vec![Permission::TrafficModify, Permission::SessionHook],
        &["http_hook", "session_action"],
    )
    .await;
    let handle = worker.handle();
    let mut req = request();
    let original = req.clone();
    let outcome = run_pre_send_plugin_hooks(
        std::slice::from_ref(&handle),
        std::slice::from_ref(&handle),
        Uuid::new_v4(),
        protocol::SourceDto::Repeater,
        &mut req,
        Instant::now() + Duration::from_secs(3),
        &CancellationToken::new(),
    )
    .await;
    assert!(matches!(
        outcome,
        PreSendHookOutcome::Proceed { mutated: true }
    ));
    assert!(req.headers.iter().any(|h| h.name == "x-mio-plugin-session"));
    assert!(req.headers.iter().any(|h| h.name == "x-mio-plugin-hook"));
    assert_eq!(req.body, original.body);

    let mut blocked = original;
    blocked.headers.push(Header {
        name: "x-mio-plugin-depth".into(),
        value: b"2".to_vec(),
    });
    let before = blocked.clone();
    let outcome = run_pre_send_plugin_hooks(
        &[],
        std::slice::from_ref(&handle),
        Uuid::new_v4(),
        protocol::SourceDto::Repeater,
        &mut blocked,
        Instant::now() + Duration::from_secs(3),
        &CancellationToken::new(),
    )
    .await;
    assert!(matches!(
        outcome,
        PreSendHookOutcome::Proceed { mutated: false }
    ));
    assert_eq!(blocked, before);
    worker.stop().await.unwrap();
    worker_thread.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_proxy_http_ws_and_pre_send_messages_bypass_plugin_hooks_without_error() {
    let root = tempfile::tempdir().unwrap();
    let (events, _rx) = mpsc::channel(128);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let (package, _trust, worker, service_rx, worker_thread) = spawn_hook_worker(
        root.path(),
        state.project,
        &["traffic.modify", "session.hook"],
        vec![Permission::TrafficModify, Permission::SessionHook],
        &["http_hook", "ws_hook", "session_action"],
    )
    .await;
    let identity = worker.identity;
    let panel = worker.initial_panel.clone();
    let manifest_modified = package.manifest_modified;
    let manifest_size = package.manifest_bytes.len() as u64;
    let entry_modified = package.modified;
    let entry_size = package.executable_size;
    let info = LoadedPlugin {
        identity,
        directory: package.directory.clone(),
        manifest: package.manifest.clone(),
        grants: vec![Permission::TrafficModify, Permission::SessionHook],
        tab_id: None,
    };
    state.plugins.instances.insert(
        identity.instance_id,
        Instance {
            worker,
            services: service_rx,
            package,
            manifest_modified,
            manifest_size,
            entry_modified,
            entry_size,
            auto_reload: false,
            hook_filter: PluginHookFilter::default(),
            hook_counters: std::sync::Arc::new(PluginHookCounters::default()),
            info,
            panel,
            resources: HashMap::new(),
        },
    );
    state.plugins.order.push(identity.instance_id);

    let (flow_hook, ws_hook) = state.build_proxy_plugin_hooks();
    let flow_hook = flow_hook.expect("flow_hook registered");
    let ws_hook = ws_hook.expect("ws_hook registered");
    let dl = Instant::now() + Duration::from_secs(3);

    // 1. Small HTTP request still runs through http_hook and gets modified.
    let small_record = FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "POST".into(),
            url: "http://127.0.0.1/small".into(),
            status: None,
            state: FlowState::Pending,
            request_bytes: 3,
            response_bytes: 0,
            started_unix_ms: 0,
            elapsed_ms: 0,
        },
        request: request(),
        response: None,
    };
    let decision = flow_hook(
        small_record.clone(),
        false,
        false,
        CancellationToken::new(),
        dl,
    )
    .await
    .unwrap();
    assert!(matches!(
        decision,
        mio_proxy::FlowHookDecision::ModifyRequest(ref req)
            if req.headers.iter().any(|h| h.name == "x-mio-plugin-hook")
    ));

    // 2. Response exceeding MAX_INLINE_BODY bypasses http_hook without error.
    let oversized_resp_record = FlowRecord {
        summary: FlowSummary {
            status: Some(200),
            response_bytes: (protocol::MAX_INLINE_BODY + 1024) as u64,
            ..small_record.summary.clone()
        },
        request: request(),
        response: Some(HttpResponse {
            status: 200,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: Vec::new(),
            body: vec![b'x'; protocol::MAX_INLINE_BODY + 1024],
            trailers: Vec::new(),
        }),
    };
    let decision = flow_hook(
        oversized_resp_record,
        true,
        false,
        CancellationToken::new(),
        dl,
    )
    .await
    .unwrap();
    assert!(matches!(decision, mio_proxy::FlowHookDecision::Pass));

    // 3. Combined req + resp each <= MAX_INLINE_BODY but exceeding MAX_FRAME_BYTES bypasses cleanly.
    let mut max_req = request();
    max_req.body = vec![b'a'; protocol::MAX_INLINE_BODY];
    let combined_overflow_record = FlowRecord {
        summary: FlowSummary {
            status: Some(200),
            request_bytes: protocol::MAX_INLINE_BODY as u64,
            response_bytes: protocol::MAX_INLINE_BODY as u64,
            ..small_record.summary.clone()
        },
        request: max_req,
        response: Some(HttpResponse {
            status: 200,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: Vec::new(),
            body: vec![b'b'; protocol::MAX_INLINE_BODY],
            trailers: Vec::new(),
        }),
    };
    let decision = flow_hook(
        combined_overflow_record,
        true,
        false,
        CancellationToken::new(),
        dl,
    )
    .await
    .unwrap();
    assert!(matches!(decision, mio_proxy::FlowHookDecision::Pass));

    // 4. WebSocket hook modifies small frames and cleanly bypasses oversized payloads.
    let ws_small = ws_hook(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "wss://127.0.0.1/ws".into(),
        mio_plugin_api::WebSocketDirection::ClientToServer,
        mio_plugin_api::WebSocketKind::Text,
        b"ping".to_vec(),
        CancellationToken::new(),
        dl,
    )
    .await
    .unwrap();
    assert!(matches!(
        ws_small,
        mio_proxy::WebSocketHookDecision::Modify(ref payload) if payload == b"ping:ws"
    ));
    let ws_large = ws_hook(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "wss://127.0.0.1/ws".into(),
        mio_plugin_api::WebSocketDirection::ServerToClient,
        mio_plugin_api::WebSocketKind::Binary,
        vec![0x42; protocol::MAX_INLINE_BODY + 1],
        CancellationToken::new(),
        dl,
    )
    .await
    .unwrap();
    assert!(matches!(ws_large, mio_proxy::WebSocketHookDecision::Pass));

    // 5. Repeater/Intruder pre_send cleanly bypasses oversized bodies without failing the send.
    let mut oversized_pre = request();
    oversized_pre.body = vec![b'z'; protocol::MAX_INLINE_BODY + 1];
    let expected_pre = oversized_pre.clone();
    let outcome = run_pre_send_plugin_hooks(
        &state.strict_session_action_hooks(),
        &state.tool_http_hooks(),
        Uuid::new_v4(),
        protocol::SourceDto::Repeater,
        &mut oversized_pre,
        dl,
        &CancellationToken::new(),
    )
    .await;
    assert!(matches!(
        outcome,
        PreSendHookOutcome::Proceed { mutated: false }
    ));
    assert_eq!(oversized_pre, expected_pre);

    state.stop_plugins().await.unwrap();
    worker_thread.join().unwrap();
}

struct FailingHookPlugin {
    fail_http: bool,
    fail_ws: bool,
    fail_session: bool,
    invocations: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl mio_plugin_sdk::MioPlugin for FailingHookPlugin {
    fn id(&self) -> &str {
        "lab.hooks"
    }

    fn version(&self) -> &str {
        "1"
    }

    fn on_http_hook(
        &mut self,
        _ctx: &mut dyn mio_plugin_sdk::HostClient,
        call: mio_plugin_sdk::protocol::HttpHookCall,
    ) -> Result<mio_plugin_sdk::protocol::HttpHookResult, String> {
        self.invocations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail_http {
            return Err("simulated http hook failure".into());
        }
        use mio_plugin_sdk::protocol::{HeaderDto, HookAction, HttpHookResult};
        if !call.response_stage {
            let mut req = call.request.ok_or("missing request")?;
            req.headers.push(HeaderDto {
                name: "x-mio-plugin-hook".into(),
                value: b"ok".to_vec(),
            });
            return Ok(HttpHookResult {
                action: HookAction::Modify as i32,
                request: Some(req),
                response: None,
            });
        }
        Ok(HttpHookResult {
            action: HookAction::Pass as i32,
            request: None,
            response: None,
        })
    }

    fn on_ws_hook(
        &mut self,
        _ctx: &mut dyn mio_plugin_sdk::HostClient,
        call: mio_plugin_sdk::protocol::WebSocketHookCall,
    ) -> Result<mio_plugin_sdk::protocol::WebSocketHookResult, String> {
        self.invocations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail_ws {
            return Err("simulated ws hook failure".into());
        }
        Ok(mio_plugin_sdk::protocol::WebSocketHookResult {
            action: mio_plugin_sdk::protocol::HookAction::Pass as i32,
            payload: call.payload,
            ..Default::default()
        })
    }

    fn on_session_action(
        &mut self,
        _ctx: &mut dyn mio_plugin_sdk::HostClient,
        call: mio_plugin_sdk::protocol::SessionActionCall,
    ) -> Result<mio_plugin_sdk::protocol::SessionActionResult, String> {
        self.invocations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail_session {
            return Err("simulated session hook failure".into());
        }
        Ok(mio_plugin_sdk::protocol::SessionActionResult {
            modified: false,
            request: call.request,
        })
    }
}

async fn spawn_failing_hook_state(
    dir: &Path,
    plugin: FailingHookPlugin,
) -> (
    State,
    mpsc::Receiver<Event>,
    ChannelIdentity,
    std::thread::JoinHandle<()>,
) {
    let (events, rx) = mpsc::channel(128);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let grant_strs = &["traffic.modify", "session.hook"];
    let grants = vec![Permission::TrafficModify, Permission::SessionHook];
    let contributions = &["http_hook", "ws_hook", "session_action"];
    write_native_hook_manifest(dir, grant_strs, contributions);
    let package = VerifiedPackage::read(dir).unwrap();
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: state.project,
    };
    let (host_tx, worker_rx) = tokio::io::duplex(64 * 1024);
    let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
    let rt_handle = tokio::runtime::Handle::current();
    let worker_thread = std::thread::spawn(move || {
        let mut plugin = plugin;
        let _ = mio_plugin_sdk::run_with_io(
            BlockingReader(worker_rx, rt_handle.clone()),
            BlockingWriter(worker_tx, rt_handle),
            &mut plugin,
        );
    });
    let grants_set: BTreeSet<Permission> = grants.iter().copied().collect();
    let (worker, service_rx) = crate::plugins::spawn_test_duplex_supervisor(
        identity,
        package.manifest.clone(),
        grants_set,
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    let panel = worker.initial_panel.clone();
    let manifest_modified = package.manifest_modified;
    let manifest_size = package.manifest_bytes.len() as u64;
    let entry_modified = package.modified;
    let entry_size = package.executable_size;
    let info = LoadedPlugin {
        identity,
        directory: package.directory.clone(),
        manifest: package.manifest.clone(),
        grants,
        tab_id: None,
    };
    state.plugins.instances.insert(
        identity.instance_id,
        Instance {
            worker,
            services: service_rx,
            package,
            manifest_modified,
            manifest_size,
            entry_modified,
            entry_size,
            auto_reload: false,
            hook_filter: PluginHookFilter::default(),
            hook_counters: std::sync::Arc::new(PluginHookCounters::default()),
            info,
            panel,
            resources: HashMap::new(),
        },
    );
    state.plugins.order.push(identity.instance_id);
    (state, rx, identity, worker_thread)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pre_filter_short_circuits_irrelevant_traffic_and_records_metrics() {
    let root = tempfile::tempdir().unwrap();
    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (mut state, mut rx, identity, worker_thread) = spawn_failing_hook_state(
        root.path(),
        FailingHookPlugin {
            fail_http: false,
            fail_ws: false,
            fail_session: false,
            invocations: std::sync::Arc::clone(&invocations),
        },
    )
    .await;

    let mut ws = state.store.metadata().workspace().unwrap();
    ws.revision += 1;
    ws.scope.include.push(ScopeRule {
        id: Uuid::new_v4(),
        enabled: true,
        prefix: "https://api.example.invalid".into(),
        include_subdomains: false,
    });
    ws.scope.include.push(ScopeRule {
        id: Uuid::new_v4(),
        enabled: true,
        prefix: "wss://api.example.invalid".into(),
        include_subdomains: false,
    });
    state
        .command(Command::SaveWorkspace {
            project: state.project,
            workspace: ws,
        })
        .await
        .unwrap();

    let op_set = Uuid::new_v4();
    let filter = PluginHookFilter {
        scope_only: true,
        request_stage: true,
        response_stage: false,
        skip_static_resources: true,
        host_allowlist: vec!["api.example.invalid".into()],
        host_denylist: vec![],
        path_allowlist: vec!["/v1/*".into()],
        path_denylist: vec!["/v1/skip".into()],
        failure_policy: PluginHookFailurePolicy::FailClosed,
    };
    state
        .plugin_command(
            state.project,
            op_set,
            PluginAction::SetHookFilter {
                instance_id: identity.instance_id,
                filter: filter.clone(),
            },
        )
        .await
        .unwrap();
    let mut updated = false;
    while let Ok(ev) = rx.try_recv() {
        if let Event::PluginOperation {
            operation_id,
            result: Ok(PluginOutcome::HookFilterUpdated { .. }),
            ..
        } = ev
            && operation_id == op_set
        {
            updated = true;
        }
    }
    assert!(updated, "SetHookFilter must emit HookFilterUpdated");

    let (flow_hook, ws_hook) = state.build_proxy_plugin_hooks();
    let flow_hook = flow_hook.unwrap();
    let ws_hook = ws_hook.unwrap();
    let dl = Instant::now() + Duration::from_secs(3);

    // Filtered out: out-of-scope host, denied path, static asset, and response stage.
    for (url, resp_stage) in [
        ("https://other.example.invalid/v1/items", false),
        ("https://api.example.invalid/v1/skip", false),
        ("https://api.example.invalid/v1/banner.png", false),
        ("https://api.example.invalid/v1/items", true),
    ] {
        let mut req = request();
        req.url = url.into();
        let rec = FlowRecord {
            summary: FlowSummary {
                id: Uuid::new_v4(),
                revision: 1,
                source: TrafficSource::Proxy,
                method: "POST".into(),
                url: url.into(),
                status: resp_stage.then_some(200),
                state: FlowState::Pending,
                request_bytes: 3,
                response_bytes: 0,
                started_unix_ms: 0,
                elapsed_ms: 0,
            },
            request: req,
            response: resp_stage.then_some(HttpResponse {
                status: 200,
                version: HttpVersion::Http11,
                reason_phrase: None,
                headers: Vec::new(),
                body: b"ok".to_vec(),
                trailers: Vec::new(),
            }),
        };
        let decision = flow_hook(rec, resp_stage, false, CancellationToken::new(), dl)
            .await
            .unwrap();
        assert!(matches!(decision, mio_proxy::FlowHookDecision::Pass));
    }
    // Filtered out WS (out of scope).
    let ws_dec = ws_hook(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "wss://other.example.invalid/v1/ws".into(),
        mio_plugin_api::WebSocketDirection::ClientToServer,
        mio_plugin_api::WebSocketKind::Text,
        b"hi".to_vec(),
        CancellationToken::new(),
        dl,
    )
    .await
    .unwrap();
    assert!(matches!(ws_dec, mio_proxy::WebSocketHookDecision::Pass));
    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "filtered traffic must short-circuit without crossing stdio IPC"
    );

    // Oversized matching request increments oversized_bypassed.
    let mut big_req = request();
    big_req.url = "https://api.example.invalid/v1/upload".into();
    big_req.body = vec![b'z'; protocol::MAX_INLINE_BODY + 1];
    let big_rec = FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "POST".into(),
            url: big_req.url.clone(),
            status: None,
            state: FlowState::Pending,
            request_bytes: big_req.body.len() as u64,
            response_bytes: 0,
            started_unix_ms: 0,
            elapsed_ms: 0,
        },
        request: big_req,
        response: None,
    };
    assert!(matches!(
        flow_hook(big_rec, false, false, CancellationToken::new(), dl)
            .await
            .unwrap(),
        mio_proxy::FlowHookDecision::Pass
    ));
    assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 0);

    // Matching request invokes worker once.
    let mut ok_req = request();
    ok_req.url = "https://api.example.invalid/v1/items".into();
    let ok_rec = FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "POST".into(),
            url: ok_req.url.clone(),
            status: None,
            state: FlowState::Pending,
            request_bytes: 3,
            response_bytes: 0,
            started_unix_ms: 0,
            elapsed_ms: 0,
        },
        request: ok_req,
        response: None,
    };
    assert!(matches!(
        flow_hook(ok_rec, false, false, CancellationToken::new(), dl)
            .await
            .unwrap(),
        mio_proxy::FlowHookDecision::ModifyRequest(_)
    ));
    assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 1);

    let op_metrics = Uuid::new_v4();
    state
        .plugin_command(state.project, op_metrics, PluginAction::QueryHookMetrics)
        .await
        .unwrap();
    let mut snapshot = None;
    while let Ok(ev) = rx.try_recv() {
        if let Event::PluginOperation {
            operation_id,
            result: Ok(PluginOutcome::HookMetrics { metrics }),
            ..
        } = ev
            && operation_id == op_metrics
        {
            snapshot = metrics
                .into_iter()
                .find(|(id, _)| *id == identity.instance_id)
                .map(|(_, m)| m);
        }
    }
    let m = snapshot.expect("HookMetrics snapshot present");
    assert_eq!(m.invoked, 1);
    assert_eq!(m.filtered_skipped, 5);
    assert_eq!(m.oversized_bypassed, 1);
    assert_eq!(m.errors_or_timeouts, 0);
    assert_eq!(m.fail_open_passes, 0);

    state.stop_plugins().await.unwrap();
    worker_thread.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fail_open_passes_errors_with_diagnostic_while_fail_closed_rejects() {
    let root = tempfile::tempdir().unwrap();
    let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (mut state, mut rx, identity, worker_thread) = spawn_failing_hook_state(
        root.path(),
        FailingHookPlugin {
            fail_http: true,
            fail_ws: true,
            fail_session: true,
            invocations,
        },
    )
    .await;

    let dl = Instant::now() + Duration::from_secs(3);
    let rec = FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "POST".into(),
            url: "https://api.example.invalid/v1/submit".into(),
            status: None,
            state: FlowState::Pending,
            request_bytes: 3,
            response_bytes: 0,
            started_unix_ms: 0,
            elapsed_ms: 0,
        },
        request: request(),
        response: None,
    };

    // 1. Default FailClosed rejects proxy HTTP, WS, and pre-send.
    let (flow_hook, ws_hook) = state.build_proxy_plugin_hooks();
    let flow_hook = flow_hook.unwrap();
    let ws_hook = ws_hook.unwrap();
    assert!(
        flow_hook(rec.clone(), false, false, CancellationToken::new(), dl)
            .await
            .is_err()
    );
    assert!(
        ws_hook(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "wss://api.example.invalid/v1/ws".into(),
            mio_plugin_api::WebSocketDirection::ClientToServer,
            mio_plugin_api::WebSocketKind::Text,
            b"hi".to_vec(),
            CancellationToken::new(),
            dl,
        )
        .await
        .is_err()
    );
    let mut pre_req = request();
    assert!(matches!(
        run_pre_send_plugin_hooks_configured(
            &state.strict_session_action_hook_entries(),
            &state.tool_http_hook_entries(),
            None,
            None,
            Uuid::new_v4(),
            protocol::SourceDto::Repeater,
            &mut pre_req,
            None,
            dl,
            &CancellationToken::new(),
        )
        .await,
        PreSendHookOutcome::Failed(_)
    ));

    // 2. Switching to FailOpen passes proxy HTTP, WS, and pre-send cleanly and records diagnostics.
    state
        .plugin_command(
            state.project,
            Uuid::new_v4(),
            PluginAction::SetHookFilter {
                instance_id: identity.instance_id,
                filter: PluginHookFilter {
                    failure_policy: PluginHookFailurePolicy::FailOpen,
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
    while rx.try_recv().is_ok() {}

    let (flow_hook, ws_hook) = state.build_proxy_plugin_hooks();
    let flow_hook = flow_hook.unwrap();
    let ws_hook = ws_hook.unwrap();
    let flow_dec = flow_hook(rec, false, false, CancellationToken::new(), dl)
        .await
        .unwrap();
    assert!(matches!(flow_dec, mio_proxy::FlowHookDecision::Pass));
    let ws_dec = ws_hook(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "wss://api.example.invalid/v1/ws".into(),
        mio_plugin_api::WebSocketDirection::ClientToServer,
        mio_plugin_api::WebSocketKind::Text,
        b"hi".to_vec(),
        CancellationToken::new(),
        dl,
    )
    .await
    .unwrap();
    assert!(matches!(ws_dec, mio_proxy::WebSocketHookDecision::Pass));
    let mut pre_open_req = request();
    let (diag_tx, mut diag_rx) = mpsc::channel(16);
    assert!(matches!(
        run_pre_send_plugin_hooks_configured(
            &state.strict_session_action_hook_entries(),
            &state.tool_http_hook_entries(),
            None,
            Some(&diag_tx),
            Uuid::new_v4(),
            protocol::SourceDto::Repeater,
            &mut pre_open_req,
            None,
            dl,
            &CancellationToken::new(),
        )
        .await,
        PreSendHookOutcome::Proceed { mutated: false }
    ));
    let mut pre_diags = 0;
    while let Ok(Event::PluginDiagnostic { message, .. }) = diag_rx.try_recv() {
        assert!(message.contains("Fail-Open"));
        pre_diags += 1;
    }
    assert_eq!(pre_diags, 2);

    let mut proxy_diags = 0;
    while let Ok(ev) = rx.try_recv() {
        if let Event::PluginDiagnostic { message, .. } = ev
            && message.contains("Fail-Open")
        {
            proxy_diags += 1;
        }
    }
    assert_eq!(proxy_diags, 2);

    let snap = state.plugins.instances[&identity.instance_id]
        .hook_counters
        .snapshot();
    assert_eq!(snap.errors_or_timeouts, 7);
    assert_eq!(snap.fail_open_passes, 4);

    state.stop_plugins().await.unwrap();
    worker_thread.join().unwrap();
}

#[derive(Clone, Copy)]
enum LargeBodyMode {
    PreserveWithRead = 0,
    RewriteSpill = 1,
    ForgedPath = 2,
}

struct LargeBodyHookPlugin {
    mode: std::sync::Arc<std::sync::atomic::AtomicU8>,
    observed_req_path: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    observed_resp_path: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    observed_ws_path: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    seen_req_len: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    seen_resp_len: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    seen_ws_len: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl mio_plugin_sdk::MioPlugin for LargeBodyHookPlugin {
    fn id(&self) -> &str {
        "lab.hooks"
    }

    fn version(&self) -> &str {
        "1"
    }

    fn supports_large_body(&self) -> bool {
        true
    }

    fn on_http_hook(
        &mut self,
        _ctx: &mut dyn mio_plugin_sdk::HostClient,
        call: mio_plugin_sdk::protocol::HttpHookCall,
    ) -> Result<mio_plugin_sdk::protocol::HttpHookResult, String> {
        use mio_plugin_sdk::protocol::{HeaderDto, HookAction, HttpHookResult};
        let mode = self.mode.load(std::sync::atomic::Ordering::SeqCst);
        if !call.response_stage {
            let mut req = call.request.ok_or("missing request")?;
            if req.body_omitted {
                *self.observed_req_path.lock().unwrap() = Some(req.body_file_path.clone());
            }
            let body = req
                .read_body_bytes()
                .map_err(|e| format!("failed reading req spill: {e}"))?;
            self.seen_req_len
                .store(body.len(), std::sync::atomic::Ordering::SeqCst);
            if mode == LargeBodyMode::PreserveWithRead as u8 {
                req.headers.push(HeaderDto {
                    name: "x-large-req-len".into(),
                    value: body.len().to_string().into_bytes(),
                });
            } else if mode == LargeBodyMode::RewriteSpill as u8 {
                let replacement = vec![b'R'; mio_plugin_sdk::protocol::MAX_INLINE_BODY + 10_000];
                drop(body);
                req.set_body_or_spill(replacement)
                    .map_err(|e| format!("failed spilling modified req: {e}"))?;
            } else if mode == LargeBodyMode::ForgedPath as u8 {
                drop(body);
                req.body.clear();
                req.body_omitted = true;
                req.original_body_len = 128;
                req.body_file_path = std::env::temp_dir()
                    .join("nonexistent-outside-spill.bin")
                    .to_string_lossy()
                    .into_owned();
            }
            return Ok(HttpHookResult {
                action: HookAction::Modify as i32,
                request: Some(req),
                response: None,
            });
        }
        let mut resp = call.response.ok_or("missing response")?;
        if resp.body_omitted {
            *self.observed_resp_path.lock().unwrap() = Some(resp.body_file_path.clone());
        }
        let body = resp
            .read_body_bytes()
            .map_err(|e| format!("failed reading resp spill: {e}"))?;
        self.seen_resp_len
            .store(body.len(), std::sync::atomic::Ordering::SeqCst);
        resp.headers.push(HeaderDto {
            name: "x-large-resp-len".into(),
            value: body.len().to_string().into_bytes(),
        });
        Ok(HttpHookResult {
            action: HookAction::Modify as i32,
            request: None,
            response: Some(resp),
        })
    }

    fn on_ws_hook(
        &mut self,
        _ctx: &mut dyn mio_plugin_sdk::HostClient,
        call: mio_plugin_sdk::protocol::WebSocketHookCall,
    ) -> Result<mio_plugin_sdk::protocol::WebSocketHookResult, String> {
        use mio_plugin_sdk::protocol::{HookAction, WebSocketHookResult};
        if call.payload_omitted {
            *self.observed_ws_path.lock().unwrap() = Some(call.payload_file_path.clone());
        }
        let payload = call
            .read_payload_bytes()
            .map_err(|e| format!("failed reading ws spill: {e}"))?;
        self.seen_ws_len
            .store(payload.len(), std::sync::atomic::Ordering::SeqCst);
        let mut updated = payload.into_owned();
        updated.extend_from_slice(b":v17");
        WebSocketHookResult::with_modified_payload_from(HookAction::Modify, &call, updated)
            .map_err(|e| format!("failed spilling ws payload: {e}"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v17_large_body_spill_preserves_rewrites_and_rejects_escape() {
    let root = tempfile::tempdir().unwrap();
    let outside_file = root.path().join("outside-secret.bin");
    fs::write(&outside_file, b"outside").unwrap();

    let mode = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
        LargeBodyMode::PreserveWithRead as u8,
    ));
    let observed_req_path = std::sync::Arc::new(std::sync::Mutex::new(None));
    let observed_resp_path = std::sync::Arc::new(std::sync::Mutex::new(None));
    let observed_ws_path = std::sync::Arc::new(std::sync::Mutex::new(None));
    let seen_req_len = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen_resp_len = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen_ws_len = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let plugin = LargeBodyHookPlugin {
        mode: std::sync::Arc::clone(&mode),
        observed_req_path: std::sync::Arc::clone(&observed_req_path),
        observed_resp_path: std::sync::Arc::clone(&observed_resp_path),
        observed_ws_path: std::sync::Arc::clone(&observed_ws_path),
        seen_req_len: std::sync::Arc::clone(&seen_req_len),
        seen_resp_len: std::sync::Arc::clone(&seen_resp_len),
        seen_ws_len: std::sync::Arc::clone(&seen_ws_len),
    };

    let (events, _rx) = mpsc::channel(128);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let plugin_dir = root.path().join("plugin");
    fs::create_dir_all(&plugin_dir).unwrap();
    let grant_strs = &["traffic.modify", "session.hook"];
    let grants = vec![Permission::TrafficModify, Permission::SessionHook];
    let contributions = &["http_hook", "ws_hook"];
    write_native_hook_manifest(&plugin_dir, grant_strs, contributions);
    let package = VerifiedPackage::read(&plugin_dir).unwrap();
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: state.project,
    };
    let (host_tx, worker_rx) = tokio::io::duplex(64 * 1024);
    let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
    let rt_handle = tokio::runtime::Handle::current();
    let worker_thread = std::thread::spawn(move || {
        let mut plugin = plugin;
        let _ = mio_plugin_sdk::run_with_io(
            BlockingReader(worker_rx, rt_handle.clone()),
            BlockingWriter(worker_tx, rt_handle),
            &mut plugin,
        );
    });
    let grants_set: BTreeSet<Permission> = grants.iter().copied().collect();
    let (worker, service_rx) = crate::plugins::spawn_test_duplex_supervisor(
        identity,
        package.manifest.clone(),
        grants_set,
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    assert!(worker.supports_large_body);
    assert!(worker.handle().supports_large_body());

    let panel = worker.initial_panel.clone();
    let manifest_modified = package.manifest_modified;
    let manifest_size = package.manifest_bytes.len() as u64;
    let entry_modified = package.modified;
    let entry_size = package.executable_size;
    let info = LoadedPlugin {
        identity,
        directory: package.directory.clone(),
        manifest: package.manifest.clone(),
        grants,
        tab_id: None,
    };
    state.plugins.instances.insert(
        identity.instance_id,
        Instance {
            worker,
            services: service_rx,
            package,
            manifest_modified,
            manifest_size,
            entry_modified,
            entry_size,
            auto_reload: false,
            hook_filter: PluginHookFilter::default(),
            hook_counters: std::sync::Arc::new(PluginHookCounters::default()),
            info,
            panel,
            resources: HashMap::new(),
        },
    );
    state.plugins.order.push(identity.instance_id);

    let (flow_hook, ws_hook) = state.build_proxy_plugin_hooks();
    let flow_hook = flow_hook.unwrap();
    let ws_hook = ws_hook.unwrap();
    let dl = Instant::now() + Duration::from_secs(5);

    // 1. 700 KiB HTTP request preserves original large body when worker edits only headers,
    // and temp spill file is cleaned up immediately once the hook finishes.
    let big_req_body = vec![b'X'; 700 * 1024];
    let mut big_req = request();
    big_req.body = big_req_body.clone();
    let rec1 = FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "POST".into(),
            url: big_req.url.clone(),
            status: None,
            state: FlowState::Pending,
            request_bytes: big_req_body.len() as u64,
            response_bytes: 0,
            started_unix_ms: 0,
            elapsed_ms: 0,
        },
        request: big_req.clone(),
        response: None,
    };
    let dec1 = flow_hook(rec1, false, false, CancellationToken::new(), dl)
        .await
        .unwrap();
    let mio_proxy::FlowHookDecision::ModifyRequest(mod_req) = dec1 else {
        panic!("expected ModifyRequest");
    };
    assert_eq!(mod_req.body, big_req_body);
    assert!(mod_req.headers.iter().any(|h| {
        h.name == "x-large-req-len" && h.value == (700 * 1024).to_string().as_bytes()
    }));
    let req_spill_path = observed_req_path.lock().unwrap().clone().unwrap();
    assert!(!Path::new(&req_spill_path).exists(), "spill file must be cleaned up");
    assert_eq!(
        seen_req_len.load(std::sync::atomic::Ordering::SeqCst),
        700 * 1024
    );

    // 2. Combined-frame overflow (512 KiB req + 512 KiB resp in response stage) spills and succeeds.
    let combined_req = HttpRequest {
        body: vec![b'P'; protocol::MAX_INLINE_BODY],
        ..request()
    };
    let combined_resp = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![],
        body: vec![b'Q'; protocol::MAX_INLINE_BODY],
        trailers: vec![],
    };
    let rec2 = FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "POST".into(),
            url: combined_req.url.clone(),
            status: Some(200),
            state: FlowState::Pending,
            request_bytes: combined_req.body.len() as u64,
            response_bytes: combined_resp.body.len() as u64,
            started_unix_ms: 0,
            elapsed_ms: 0,
        },
        request: combined_req,
        response: Some(combined_resp.clone()),
    };
    let dec2 = flow_hook(rec2, true, false, CancellationToken::new(), dl)
        .await
        .unwrap();
    let mio_proxy::FlowHookDecision::ModifyResponse(mod_resp) = dec2 else {
        panic!("expected ModifyResponse on combined overflow");
    };
    assert_eq!(mod_resp.body, combined_resp.body);
    assert!(mod_resp.headers.iter().any(|h| {
        h.name == "x-large-resp-len" && h.value == protocol::MAX_INLINE_BODY.to_string().as_bytes()
    }));

    // 3. Rewriting a >512 KiB body via sibling spill returns the new body bytes cleanly.
    mode.store(
        LargeBodyMode::RewriteSpill as u8,
        std::sync::atomic::Ordering::SeqCst,
    );
    let rec3 = FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "POST".into(),
            url: big_req.url.clone(),
            status: None,
            state: FlowState::Pending,
            request_bytes: big_req.body.len() as u64,
            response_bytes: 0,
            started_unix_ms: 0,
            elapsed_ms: 0,
        },
        request: big_req.clone(),
        response: None,
    };
    let dec3 = flow_hook(rec3, false, false, CancellationToken::new(), dl)
        .await
        .unwrap();
    let mio_proxy::FlowHookDecision::ModifyRequest(rewritten_req) = dec3 else {
        panic!("expected ModifyRequest");
    };
    assert_eq!(
        rewritten_req.body,
        vec![b'R'; protocol::MAX_INLINE_BODY + 10_000]
    );

    // 4. WebSocket >512 KiB payload spill + sibling modification succeeds and cleans up.
    let big_ws = vec![b'W'; 600 * 1024];
    let ws_dec = ws_hook(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "wss://api.example.invalid/ws".into(),
        mio_plugin_api::WebSocketDirection::ClientToServer,
        mio_plugin_api::WebSocketKind::Binary,
        big_ws.clone(),
        CancellationToken::new(),
        dl,
    )
    .await
    .unwrap();
    let mut expected_ws = big_ws;
    expected_ws.extend_from_slice(b":v17");
    let mio_proxy::WebSocketHookDecision::Modify(mod_ws) = ws_dec else {
        panic!("expected WS Modify");
    };
    assert_eq!(mod_ws, expected_ws);
    let ws_spill_path = observed_ws_path.lock().unwrap().clone().unwrap();
    assert!(!Path::new(&ws_spill_path).exists());

    // 5. Forged spill path outside the per-call spill dir is rejected cleanly.
    mode.store(
        LargeBodyMode::ForgedPath as u8,
        std::sync::atomic::Ordering::SeqCst,
    );
    let rec5 = FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "POST".into(),
            url: big_req.url.clone(),
            status: None,
            state: FlowState::Pending,
            request_bytes: big_req.body.len() as u64,
            response_bytes: 0,
            started_unix_ms: 0,
            elapsed_ms: 0,
        },
        request: big_req,
        response: None,
    };
    assert!(
        flow_hook(rec5, false, false, CancellationToken::new(), dl)
            .await
            .is_err(),
        "forged spill path outside per-call dir must be rejected"
    );

    let counters = state.plugins.instances[&identity.instance_id]
        .hook_counters
        .snapshot();
    assert_eq!(counters.invoked, 5);
    assert_eq!(counters.oversized_bypassed, 0);
    assert_eq!(counters.errors_or_timeouts, 1);

    state.stop_plugins().await.unwrap();
    worker_thread.join().unwrap();
}
