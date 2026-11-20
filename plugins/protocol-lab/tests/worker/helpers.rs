use base64::{Engine, engine::general_purpose::STANDARD};

use super::*;

pub(super) fn project() -> ProjectKey {
    ProjectKey {
        id: Uuid::new_v4(),
        generation: 1,
    }
}

pub(super) fn package(name: &str, binary: &str, fixture: bool) -> (TempDir, VerifiedPackage) {
    let temporary = tempfile::tempdir().unwrap();
    fs::create_dir(temporary.path().join("bin")).unwrap();
    let bin_name = if cfg!(windows) && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    fs::copy(binary, temporary.path().join("bin").join(&bin_name)).unwrap();
    let manifest = serde_json::json!({"id": if fixture {"lab.fixture"} else {"lab.protocol"}, "version":"0.1.0", "api_version":1,
        "engine":"native", "entry":format!("bin/{bin_name}"), "permissions":["traffic.read","draft.write","repeater.open","network.send"],
        "contributions":["suite_panel","message_editor"]});
    fs::write(
        temporary.path().join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let package = VerifiedPackage::read(temporary.path()).unwrap();
    (temporary, package)
}

pub(super) fn real_package() -> (TempDir, VerifiedPackage) {
    let bin = std::env::var("MIO_TEST_PROTOCOL_LAB_BIN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_protocol-lab").into());
    package("protocol-lab", &bin, false)
}
pub(super) fn fixture_package(name: &str) -> (TempDir, VerifiedPackage) {
    let bin = std::env::var("MIO_TEST_PROTOCOL_LAB_WORKER_BIN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_protocol-lab-test-worker").into());
    package(name, &bin, true)
}

pub(super) async fn start(
    package: VerifiedPackage,
    grants: &[Permission],
) -> (PluginSupervisor, mpsc::Receiver<ServiceRequest>) {
    let (services, calls) = mpsc::channel(16);
    let trust = NativeTrust::confirmed(&package, grants.iter().copied()).unwrap();
    let worker = timeout(
        QUICK,
        PluginSupervisor::start(package, project(), trust, services),
    )
    .await
    .unwrap()
    .unwrap();
    (worker, calls)
}

pub(super) async fn panel(
    worker: &mut PluginSupervisor,
    matches: impl Fn(&PanelSnapshot) -> bool,
) -> Arc<PanelSnapshot> {
    timeout(QUICK, async {
        loop {
            match worker.events.recv().await.expect("worker events") {
                PluginEvent::Panel { snapshot, .. } if matches(&snapshot) => return snapshot,
                PluginEvent::Error { failure, .. } => panic!("worker error: {}", failure.message),
                PluginEvent::Stopped { reason, .. } => panic!("worker stopped: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("causal panel update")
}

pub(super) fn root(panel: &PanelSnapshot) -> &Node {
    panel.root.as_ref().unwrap()
}

pub(super) fn editor(panel: &PanelSnapshot, id: u64) -> &mio_plugin_api::panel::Editor {
    let Some(node::Kind::Editor(editor)) = root(panel).find(id).and_then(|n| n.kind.as_ref())
    else {
        panic!("missing editor {id}");
    };
    editor
}

pub(super) fn snapshots(panel: &PanelSnapshot) -> Vec<EditorSnapshot> {
    let mut editors = vec![];
    root(panel).editors(&mut editors);
    editors
        .into_iter()
        .map(|(_, e)| EditorSnapshot {
            document_id: e.document_id,
            revision: e.revision,
            contents: e.contents.clone(),
            selection_start: 0,
            selection_end: 0,
        })
        .collect()
}

pub(super) fn click(id: u64, editors: Vec<EditorSnapshot>) -> UiEvent {
    UiEvent {
        node_id: id,
        event: Some(ui_event::Event::Click(Empty {})),
        editors,
    }
}

pub(super) fn request(url: String) -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        url,
        version: HttpVersion::Http11,
        headers: vec![
            Header {
                name: "Content-Length".into(),
                value: b"8".to_vec(),
            },
            Header {
                name: "X-Duplicate".into(),
                value: b"first".to_vec(),
            },
            Header {
                name: "X-Duplicate".into(),
                value: b"second".to_vec(),
            },
        ],
        body: STANDARD.encode("基线".as_bytes()).into_bytes(),
        trailers: vec![],
    }
}

pub(super) async fn next_call(calls: &mut mpsc::Receiver<ServiceRequest>) -> ServiceRequest {
    timeout(QUICK, calls.recv())
        .await
        .unwrap()
        .expect("service call")
}

pub(super) async fn entered_hang(
    worker: &PluginSupervisor,
    calls: &mut mpsc::Receiver<ServiceRequest>,
) -> (ServiceRequest, u32) {
    worker.handle().try_event(click(1, vec![])).unwrap();
    let request = next_call(calls).await;
    let ServiceOperation::QueryHistory { filter, .. } = &request.operation else {
        panic!("causal hang notification");
    };
    let pid = filter
        .strip_prefix("entered-hang:")
        .unwrap()
        .parse()
        .unwrap();
    (request, pid)
}

pub(super) fn assert_reaped(pid: u32) {
    #[cfg(unix)]
    {
        let status = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .unwrap();
        assert!(
            !status.status.success() || String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "worker still exists after join: {pid}"
        );
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
            fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
            fn CloseHandle(handle: isize) -> i32;
        }
        const SYNCHRONIZE: u32 = 0x0010_0000;
        const WAIT_OBJECT_0: u32 = 0;
        let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
        if handle != 0 {
            let wait = unsafe { WaitForSingleObject(handle, 500) };
            unsafe {
                let _ = CloseHandle(handle);
            }
            assert_eq!(
                wait, WAIT_OBJECT_0,
                "worker still running after join: {pid}"
            );
        }
    }
}
