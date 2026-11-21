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
    let (package, trust, worker, service_rx, worker_thread) = spawn_hook_worker(
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
            trust,
            info,
            panel,
            source: None,
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
