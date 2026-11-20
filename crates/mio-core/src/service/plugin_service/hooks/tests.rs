use super::*;
use crate::plugins::wasm::build_demo_wasm_module;
use std::{fs, path::Path};

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

fn write_demo(dir: &Path, grants: &[&str], contributions: &[&str]) {
    fs::write(dir.join("hook.wasm"), build_demo_wasm_module()).unwrap();
    let manifest = serde_json::json!({
        "id": "lab.hooks",
        "version": "1",
        "api_version": 1,
        "engine": "wasm",
        "entry": "hook.wasm",
        "permissions": grants,
        "contributions": contributions,
    });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
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

#[tokio::test]
async fn http_only_plugins_trigger_intruder_pre_send_without_session_permission() {
    let root = tempfile::tempdir().unwrap();
    write_demo(root.path(), &["traffic.modify"], &["http_hook"]);
    let package = VerifiedPackage::read(root.path()).unwrap();
    let trust = NativeTrust::confirmed(&package, [Permission::TrafficModify]).unwrap();
    let (events, _rx) = mpsc::channel(128);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let (services, service_rx) = mpsc::channel(16);
    let worker = PluginSupervisor::start(package.clone(), state.project, trust.clone(), services)
        .await
        .unwrap();
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
            .any(|h| { h.name == "x-mio-wasm-hook" && h.value == b"sandboxed" })
    );
    assert!(!req.headers.iter().any(|h| h.name == "x-mio-wasm-session"));
    assert_eq!(req.body, [0, 0xff, b'x']);
    state.stop_plugins().await.unwrap();
}

#[tokio::test]
async fn session_and_http_hooks_run_in_order_for_repeater_and_depth_skip_is_nondestructive() {
    let root = tempfile::tempdir().unwrap();
    write_demo(
        root.path(),
        &["traffic.modify", "session.hook"],
        &["http_hook", "session_action"],
    );
    let package = VerifiedPackage::read(root.path()).unwrap();
    let trust = NativeTrust::confirmed(
        &package,
        [Permission::TrafficModify, Permission::SessionHook],
    )
    .unwrap();
    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: 1,
    };
    let (services, _service_rx) = mpsc::channel(16);
    let mut worker = PluginSupervisor::start(package, project, trust, services)
        .await
        .unwrap();
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
    assert!(req.headers.iter().any(|h| h.name == "x-mio-wasm-session"));
    assert!(req.headers.iter().any(|h| h.name == "x-mio-wasm-hook"));
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
}
