use std::{fs, sync::OnceLock};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use mio_plugin_api::panel::{Empty, UiEvent, ui_event};

use super::*;

#[cfg(windows)]
const NATIVE_MAGIC: [u8; 4] = [b'M', b'Z', 0, 0];
#[cfg(not(windows))]
const NATIVE_MAGIC: [u8; 4] = [0xcf, 0xfa, 0xed, 0xfe];

fn native_entry_name() -> &'static str {
    if cfg!(windows) {
        "worker.exe"
    } else {
        "worker"
    }
}

const NATIVE_TEST_WORKER_SRC: &str = r#"
use std::fs;
use std::io::{self, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

fn tag_varint(field: u32, val: u64, out: &mut Vec<u8>) {
    varint((field as u64) << 3, out);
    varint(val, out);
}

fn tag_bytes(field: u32, data: &[u8], out: &mut Vec<u8>) {
    varint(((field as u64) << 3) | 2, out);
    varint(data.len() as u64, out);
    out.extend_from_slice(data);
}

fn read_varint(buf: &[u8], pos: &mut usize) -> u64 {
    let mut val = 0u64;
    let mut shift = 0u32;
    while *pos < buf.len() {
        let b = buf[*pos];
        *pos += 1;
        val |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    val
}

fn skip_field(wire: u64, buf: &[u8], pos: &mut usize) {
    match wire {
        0 => {
            let _ = read_varint(buf, pos);
        }
        1 => *pos = (*pos + 8).min(buf.len()),
        2 => {
            let len = read_varint(buf, pos) as usize;
            *pos = (*pos + len).min(buf.len());
        }
        5 => *pos = (*pos + 4).min(buf.len()),
        _ => *pos = buf.len(),
    }
}

fn read_frame(r: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut prefix = [0u8; 4];
    match r.read_exact(&mut prefix) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(prefix) as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(buf))
}

fn write_frame(w: &mut impl Write, msg: &[u8]) -> io::Result<()> {
    w.write_all(&(msg.len() as u32).to_be_bytes())?;
    w.write_all(msg)?;
    w.flush()
}

fn json_field(json: &str, key: &str) -> String {
    let needle = format!("\"{key}\"");
    let Some(idx) = json.find(&needle) else {
        return String::new();
    };
    let rest = &json[idx + needle.len()..];
    let Some(colon) = rest.find(':') else {
        return String::new();
    };
    let after = rest[colon + 1..].trim_start();
    let Some(stripped) = after.strip_prefix('"') else {
        return String::new();
    };
    let mut out = String::new();
    let mut chars = stripped.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => break,
            '\\' => {
                if let Some(esc) = chars.next() {
                    out.push(esc);
                }
            }
            _ => out.push(ch),
        }
    }
    out
}

fn panel_snapshot(rev: u64, title: &str) -> Vec<u8> {
    let mut btn = Vec::new();
    tag_bytes(1, b"save", &mut btn);
    tag_varint(2, 1, &mut btn);
    let mut node = Vec::new();
    tag_varint(1, 10, &mut node);
    tag_bytes(2, b"save", &mut node);
    tag_bytes(15, &btn, &mut node);
    let mut snap = Vec::new();
    tag_varint(1, rev, &mut snap);
    tag_bytes(2, title.as_bytes(), &mut snap);
    tag_bytes(3, &node, &mut snap);
    snap
}

fn encode_env(
    req_id: u64,
    instance_id: &[u8],
    project_id: &[u8],
    project_gen: u64,
    rev: u64,
    in_reply_to: u64,
    body_field: u32,
    body_bytes: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    tag_varint(1, 1, &mut out);
    tag_varint(2, 6, &mut out);
    tag_varint(3, req_id, &mut out);
    tag_bytes(4, instance_id, &mut out);
    tag_bytes(5, project_id, &mut out);
    tag_varint(6, project_gen, &mut out);
    tag_varint(7, 1, &mut out);
    tag_varint(8, rev, &mut out);
    tag_varint(9, unix_ms().saturating_add(5000), &mut out);
    if in_reply_to != 0 {
        tag_varint(10, in_reply_to, &mut out);
    }
    tag_bytes(body_field, body_bytes, &mut out);
    out
}

#[derive(Default)]
struct EnvMeta {
    req_id: u64,
    in_reply_to: u64,
    instance_id: Vec<u8>,
    project_id: Vec<u8>,
    project_gen: u64,
    has_ui_event: bool,
    has_host_result: bool,
    has_shutdown: bool,
}

fn parse_env(buf: &[u8]) -> EnvMeta {
    let mut meta = EnvMeta::default();
    let mut pos = 0;
    while pos < buf.len() {
        let key = read_varint(buf, &mut pos);
        let field = (key >> 3) as u32;
        let wire = key & 7;
        match (field, wire) {
            (3, 0) => meta.req_id = read_varint(buf, &mut pos),
            (4, 2) => {
                let len = read_varint(buf, &mut pos) as usize;
                let end = (pos + len).min(buf.len());
                meta.instance_id = buf[pos..end].to_vec();
                pos = end;
            }
            (5, 2) => {
                let len = read_varint(buf, &mut pos) as usize;
                let end = (pos + len).min(buf.len());
                meta.project_id = buf[pos..end].to_vec();
                pos = end;
            }
            (6, 0) => meta.project_gen = read_varint(buf, &mut pos),
            (10, 0) => meta.in_reply_to = read_varint(buf, &mut pos),
            (25, 2) => {
                meta.has_ui_event = true;
                skip_field(wire, buf, &mut pos);
            }
            (30, 2) => {
                meta.has_host_result = true;
                skip_field(wire, buf, &mut pos);
            }
            (32, 2) => {
                meta.has_shutdown = true;
                skip_field(wire, buf, &mut pos);
            }
            _ => skip_field(wire, buf, &mut pos),
        }
    }
    meta
}

fn main() -> io::Result<()> {
    let manifest_json = fs::read_to_string("manifest.json").unwrap_or_default();
    let plugin_id = json_field(&manifest_json, "id");
    let plugin_version = json_field(&manifest_json, "version");

    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();

    let Some(hello_buf) = read_frame(&mut stdin)? else {
        return Ok(());
    };
    let hello = parse_env(&hello_buf);

    let mut reg = Vec::new();
    tag_bytes(1, plugin_id.as_bytes(), &mut reg);
    tag_bytes(2, plugin_version.as_bytes(), &mut reg);
    tag_bytes(4, &panel_snapshot(1, &plugin_id), &mut reg);
    let mut next_req = 1u64;
    let mut rev = 1u64;
    let reg_env = encode_env(
        next_req,
        &hello.instance_id,
        &hello.project_id,
        hello.project_gen,
        rev,
        hello.req_id,
        21,
        &reg,
    );
    write_frame(&mut stdout, &reg_env)?;

    while let Some(buf) = read_frame(&mut stdin)? {
        let env = parse_env(&buf);
        if env.has_shutdown {
            break;
        }
        if env.has_ui_event {
            next_req += 1;
            let call_id = next_req;
            let mut pref_entry = Vec::new();
            tag_bytes(1, b"native_tag", &mut pref_entry);
            tag_bytes(2, b"native-v1", &mut pref_entry);
            let mut host_call = Vec::new();
            tag_bytes(10, &pref_entry, &mut host_call);
            tag_varint(20, env.req_id, &mut host_call);
            let call_env = encode_env(
                call_id,
                &hello.instance_id,
                &hello.project_id,
                hello.project_gen,
                rev,
                0,
                29,
                &host_call,
            );
            write_frame(&mut stdout, &call_env)?;

            while let Some(reply_buf) = read_frame(&mut stdin)? {
                let reply = parse_env(&reply_buf);
                if reply.has_shutdown {
                    return Ok(());
                }
                if reply.has_host_result && reply.in_reply_to == call_id {
                    break;
                }
            }

            rev += 1;
            next_req += 1;
            let snap_env = encode_env(
                next_req,
                &hello.instance_id,
                &hello.project_id,
                hello.project_gen,
                rev,
                env.req_id,
                23,
                &panel_snapshot(rev, &plugin_id),
            );
            write_frame(&mut stdout, &snap_env)?;
        }
    }
    Ok(())
}
"#;

fn native_test_worker_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("worker.rs");
        let bin = tmp.path().join(native_entry_name());
        fs::write(&src, NATIVE_TEST_WORKER_SRC).unwrap();
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let status = std::process::Command::new(rustc)
            .args([
                "--edition=2024",
                "-C",
                "opt-level=0",
                "-C",
                "debuginfo=0",
                "-o",
            ])
            .arg(&bin)
            .arg(&src)
            .status()
            .expect("spawn rustc for native test worker");
        assert!(status.success(), "rustc failed compiling native test worker");
        fs::read(&bin).unwrap()
    })
}

pub(super) fn write_native_package(
    dir: &std::path::Path,
    id: &str,
    version: &str,
    permissions: &[&str],
    contributions: &[&str],
) {
    let entry_name = native_entry_name();
    let entry_path = dir.join(entry_name);
    let worker_bytes = native_test_worker_bytes();
    if fs::read(&entry_path).ok().as_deref() != Some(worker_bytes) {
        let tmp_entry = dir.join(format!(".{entry_name}.tmp"));
        fs::write(&tmp_entry, worker_bytes).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&tmp_entry, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&tmp_entry, &entry_path).unwrap();
    }
    let manifest = serde_json::json!({
        "id": id,
        "version": version,
        "api_version": 1,
        "engine": "native",
        "entry": entry_name,
        "permissions": permissions,
        "contributions": contributions,
    });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

fn write_native_stub_package(
    dir: &std::path::Path,
    id: &str,
    version: &str,
    permissions: &[&str],
    contributions: &[&str],
) {
    let entry_name = native_entry_name();
    let entry_path = dir.join(entry_name);
    fs::write(&entry_path, NATIVE_MAGIC).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&entry_path, fs::Permissions::from_mode(0o755)).unwrap();
    let manifest = serde_json::json!({
        "id": id,
        "version": version,
        "api_version": 1,
        "engine": "native",
        "entry": entry_name,
        "permissions": permissions,
        "contributions": contributions,
    });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

pub(super) async fn drain_until_plugin_op(
    state: &mut State,
    events: &mut mpsc::Receiver<Event>,
    target_op: Uuid,
) -> Result<PluginOutcome, String> {
    loop {
        while let Ok(ev) = events.try_recv() {
            if let Event::PluginOperation {
                operation_id,
                result,
                ..
            } = ev
                && operation_id == target_op
            {
                return result;
            }
        }
        let signal = state.plugins.next_signal().await;
        state.plugin_signal(signal).await;
    }
}

#[test]
fn runtime_start_with_preferences_persists_plugin_prefs_across_project_switches() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let canonical = root.path().canonicalize().unwrap();
    let user_pref_path = canonical.join("profile").join("preferences.json");
    let plugin_pref_path = canonical.join("profile").join("plugin-preferences.json");

    let pkg_dir = canonical.join("pkg-prefs");
    fs::create_dir_all(&pkg_dir).unwrap();
    write_native_package(
        &pkg_dir,
        "lab.prefs",
        "0.1.0",
        &["storage.kv"],
        &["suite_panel"],
    );

    let (runtime, mut client) = Runtime::start_with_preferences(user_pref_path.clone()).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let project = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client.events.recv().await {
                break project;
            }
        };
        let insp_op = Uuid::new_v4();
        client
            .commands
            .send(Command::Plugin {
                project,
                operation_id: insp_op,
                action: PluginAction::Inspect {
                    directory: pkg_dir.clone(),
                },
            })
            .await
            .unwrap();
        let inspection_id = loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::Inspected(info)),
                ..
            }) = client.events.recv().await
                && operation_id == insp_op
            {
                break info.id;
            }
        };
        let load_op = Uuid::new_v4();
        client
            .commands
            .send(Command::Plugin {
                project,
                operation_id: load_op,
                action: PluginAction::Load {
                    inspection_id,
                    native_trusted: true,
                    grants: vec![Permission::Storage],
                    replace_instance: None,
                },
            })
            .await
            .unwrap();
        let (instance_id, panel_rev) = loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::Loaded { info, panel }),
                ..
            }) = client.events.recv().await
                && operation_id == load_op
            {
                break (info.identity.instance_id, panel.revision);
            }
        };
        client
            .commands
            .send(Command::Plugin {
                project,
                operation_id: Uuid::new_v4(),
                action: PluginAction::Ui {
                    instance_id,
                    panel_revision: panel_rev,
                    event_seq: 0,
                    event: UiEvent {
                        node_id: 10,
                        event: Some(ui_event::Event::Click(Empty {})),
                        editors: vec![],
                    },
                },
            })
            .await
            .unwrap();
        loop {
            if let Some(Event::PluginPanel { snapshot, .. }) = client.events.recv().await
                && snapshot.revision > panel_rev
            {
                break;
            }
        }
        assert!(
            plugin_pref_path.is_file(),
            "PreferencesSet must persist to sibling plugin-preferences.json"
        );

        client
            .commands
            .send(Command::NewProject { path: None })
            .await
            .unwrap();
        let next_project = loop {
            if let Some(Event::ProjectOpened { project: p2, .. }) = client.events.recv().await
                && p2.generation > project.generation
            {
                break p2;
            }
        };
        assert_ne!(next_project.id, project.id);
    });
    drop(client);
    runtime.shutdown().unwrap();

    let reopened = PluginPreferencesStore::new(Some(plugin_pref_path));
    assert_eq!(
        reopened.get("lab.prefs", "native_tag").unwrap().as_deref(),
        Some(&b"native-v1"[..])
    );
}

#[tokio::test]
async fn reload_at_max_instances_succeeds_and_auto_reload_enforces_permission_subset() {
    let dir = tempfile::tempdir().unwrap();
    write_native_package(
        dir.path(),
        "lab.autoreload",
        "0.1.0",
        &["traffic.read"],
        &["suite_panel"],
    );
    let (tx, mut rx) = mpsc::channel(128);
    let mut state = State::new(tx, CancellationToken::new()).unwrap();

    let insp_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            insp_op,
            PluginAction::Inspect {
                directory: dir.path().to_owned(),
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Inspected(insp) = drain_until_plugin_op(&mut state, &mut rx, insp_op)
        .await
        .unwrap()
    else {
        panic!("expected inspected");
    };
    let load_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            load_op,
            PluginAction::Load {
                inspection_id: insp.id,
                native_trusted: true,
                grants: vec![Permission::TrafficRead],
                replace_instance: None,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Loaded { info, .. } = drain_until_plugin_op(&mut state, &mut rx, load_op)
        .await
        .unwrap()
    else {
        panic!("expected loaded");
    };
    let first_id = info.identity.instance_id;

    let ar_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            ar_op,
            PluginAction::SetAutoReload {
                instance_id: first_id,
                enabled: true,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::AutoReloadUpdated {
        instance_id: ar_id,
        enabled: true,
    } = drain_until_plugin_op(&mut state, &mut rx, ar_op)
        .await
        .unwrap()
    else {
        panic!("expected AutoReloadUpdated(true)");
    };
    assert_eq!(ar_id, first_id);

    // Fill dummy instances up to MAX_INSTANCES (16) and verify Reload still succeeds.
    let mut filler_guards = Vec::new();
    while state.plugins.instances.len() < MAX_INSTANCES {
        let filler_dir = tempfile::tempdir().unwrap();
        write_native_stub_package(
            filler_dir.path(),
            "lab.filler",
            "1.0.0",
            &["traffic.read"],
            &["suite_panel"],
        );
        let pkg = VerifiedPackage::read(filler_dir.path()).unwrap();
        let id = ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: state.project,
        };
        let panel = std::sync::Arc::new(PanelSnapshot {
            revision: 1,
            title: "filler".into(),
            root: None,
        });
        let (worker, _q) = crate::plugins::test_supervisor(id, pkg.manifest.clone(), panel.clone());
        let (_stx, srx) = mpsc::channel(16);
        let m_mod = pkg.manifest_modified;
        let m_sz = pkg.manifest_bytes.len() as u64;
        let e_mod = pkg.modified;
        let e_sz = pkg.executable_size;
        filler_guards.push((_q, _stx, filler_dir));
        state.plugins.instances.insert(
            id.instance_id,
            Instance {
                worker,
                services: srx,
                info: LoadedPlugin {
                    identity: id,
                    directory: pkg.directory.clone(),
                    manifest: pkg.manifest.clone(),
                    grants: vec![Permission::TrafficRead],
                    tab_id: None,
                },
                package: pkg,
                manifest_modified: m_mod,
                manifest_size: m_sz,
                entry_modified: e_mod,
                entry_size: e_sz,
                auto_reload: false,
                panel,
                resources: HashMap::new(),
            },
        );
    }
    assert_eq!(state.plugins.instances.len(), MAX_INSTANCES);
    assert!(state.plugin_capacity().is_err());

    let reload_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            reload_op,
            PluginAction::Reload {
                instance_id: first_id,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Reloaded {
        old_instance,
        info: reloaded_info,
        ..
    } = drain_until_plugin_op(&mut state, &mut rx, reload_op)
        .await
        .unwrap()
    else {
        panic!("expected reloaded at MAX_INSTANCES");
    };
    assert_eq!(old_instance, first_id);
    let second_id = reloaded_info.identity.instance_id;
    assert_ne!(second_id, first_id);
    assert_eq!(state.plugins.instances.len(), MAX_INSTANCES);

    // Auto-reload detects safe version update on disk.
    assert!(state.check_auto_reload().is_empty());
    write_native_package(
        dir.path(),
        "lab.autoreload",
        "0.2.0-next",
        &["traffic.read"],
        &["suite_panel"],
    );
    let auto_ops = state.check_auto_reload();
    assert_eq!(auto_ops.len(), 1);
    let PluginOutcome::Reloaded { info: v2_info, .. } =
        drain_until_plugin_op(&mut state, &mut rx, auto_ops[0])
            .await
            .unwrap()
    else {
        panic!("expected auto-reload completion");
    };
    assert_eq!(v2_info.manifest.version, "0.2.0-next");
    let third_id = v2_info.identity.instance_id;

    // Auto-reload blocks silent permission escalation.
    write_native_package(
        dir.path(),
        "lab.autoreload",
        "0.3.0",
        &["traffic.read", "network.send"],
        &["suite_panel"],
    );
    let esc_ops = state.check_auto_reload();
    assert_eq!(esc_ops.len(), 1);
    let esc_err = drain_until_plugin_op(&mut state, &mut rx, esc_ops[0])
        .await
        .unwrap_err();
    assert!(esc_err.contains("拒绝热重载提权"), "{esc_err}");
    assert!(state.plugins.instances.contains_key(&third_id));
    state.stop_plugins().await.unwrap();
}

fn build_native_tar_gz(id: &str, version: &str) -> Vec<u8> {
    use std::io::Write;

    use flate2::{Compression, write::GzEncoder};

    let entry_name = native_entry_name();
    let worker = native_test_worker_bytes();
    let manifest = serde_json::json!({
        "id": id,
        "version": version,
        "api_version": 1,
        "engine": "native",
        "entry": entry_name,
        "permissions": ["traffic.read"],
        "contributions": ["suite_panel"],
    })
    .to_string();
    let mut tar = Vec::new();
    for (name, mode, data) in [
        ("manifest.json", 0o644u32, manifest.as_bytes()),
        (entry_name, 0o755u32, worker),
    ] {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        let write_oct = |dst: &mut [u8], val: u64, width: usize| {
            let s = format!("{val:0width$o}", width = width);
            dst[..width].copy_from_slice(s.as_bytes());
        };
        write_oct(&mut header[100..108], u64::from(mode), 7);
        write_oct(&mut header[108..116], 0, 7);
        write_oct(&mut header[116..124], 0, 7);
        write_oct(&mut header[124..136], data.len() as u64, 11);
        write_oct(&mut header[136..148], 0, 11);
        header[148..156].fill(b' ');
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let chk: u32 = header.iter().map(|&b| u32::from(b)).sum();
        let chk_s = format!("{chk:06o}\0 ");
        header[148..156].copy_from_slice(chk_s.as_bytes());
        tar.extend_from_slice(&header);
        tar.extend_from_slice(data);
        let rem = data.len() % 512;
        if rem != 0 {
            tar.resize(tar.len() + (512 - rem), 0);
        }
    }
    tar.resize(tar.len() + 1024, 0);
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

#[test]
fn persistent_plugins_survive_runtime_restart_toggle_disable_extensions_and_uninstall() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let canonical = root.path().canonicalize().unwrap();
    let profile_dir = canonical.join("profile");
    let user_pref_path = profile_dir.join("preferences.json");
    let plugins_dir = profile_dir.join("plugins");
    let archive_path = canonical.join("downloads").join("lab.persist.tar.gz");
    fs::create_dir_all(archive_path.parent().unwrap()).unwrap();
    fs::write(&archive_path, build_native_tar_gz("lab.persist", "0.1.0")).unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // 1. First runtime: inspect & load from single-file .tar.gz, then delete source archive.
    let (runtime1, mut client1) = Runtime::start_with_preferences(user_pref_path.clone()).unwrap();
    rt.block_on(async {
        let project = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client1.events.recv().await {
                break project;
            }
        };
        let insp_op = Uuid::new_v4();
        client1
            .commands
            .send(Command::Plugin {
                project,
                operation_id: insp_op,
                action: PluginAction::Inspect {
                    directory: archive_path.clone(),
                },
            })
            .await
            .unwrap();
        let inspection_id = loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::Inspected(info)),
                ..
            }) = client1.events.recv().await
                && operation_id == insp_op
            {
                break info.id;
            }
        };
        let load_op = Uuid::new_v4();
        client1
            .commands
            .send(Command::Plugin {
                project,
                operation_id: load_op,
                action: PluginAction::Load {
                    inspection_id,
                    native_trusted: true,
                    grants: vec![Permission::TrafficRead],
                    replace_instance: None,
                },
            })
            .await
            .unwrap();
        loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::Loaded { info, .. }),
                ..
            }) = client1.events.recv().await
                && operation_id == load_op
            {
                assert_eq!(info.manifest.id, "lab.persist");
                break;
            }
        }
    });
    drop(client1);
    runtime1.shutdown().unwrap();

    fs::remove_file(&archive_path).unwrap();
    assert!(plugins_dir.join("installed.json").is_file());
    assert!(plugins_dir.join("packages/lab.persist/manifest.json").is_file());

    // 2. Second runtime: StartProject auto-loads persisted plugin even though original .tar.gz is gone.
    let (runtime2, mut client2) = Runtime::start_with_preferences(user_pref_path.clone()).unwrap();
    rt.block_on(async {
        let initial_project = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client2.events.recv().await {
                break project;
            }
        };
        client2
            .commands
            .send(Command::StartProject {
                path: None,
                create_new: true,
                options: ProjectStartOptions::default(),
            })
            .await
            .unwrap();
        let p2 = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client2.events.recv().await
                && project.generation > initial_project.generation
            {
                break project;
            }
        };
        let mut saw_registry = false;
        let auto_instance = loop {
            match client2.events.recv().await.unwrap() {
                Event::InstalledPlugins {
                    project,
                    plugins_dir: dir,
                    extensions_disabled,
                    plugins,
                } if project == p2 => {
                    assert_eq!(dir, plugins_dir);
                    assert!(!extensions_disabled);
                    assert_eq!(plugins.len(), 1);
                    assert_eq!(plugins[0].id, "lab.persist");
                    assert!(plugins[0].enabled);
                    saw_registry = true;
                }
                Event::PluginAutoLoaded { project, info, .. } if project == p2 => {
                    assert_eq!(info.manifest.id, "lab.persist");
                    break info.identity.instance_id;
                }
                _ => {}
            }
        };
        assert!(saw_registry);

        // Uncheck Loaded -> PluginAction::Disable keeps package in installed.json with enabled=false.
        let disable_op = Uuid::new_v4();
        client2
            .commands
            .send(Command::Plugin {
                project: p2,
                operation_id: disable_op,
                action: PluginAction::Disable {
                    instance_id: auto_instance,
                },
            })
            .await
            .unwrap();
        loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::Disabled { .. }),
                ..
            }) = client2.events.recv().await
                && operation_id == disable_op
            {
                break;
            }
        }
    });
    drop(client2);
    runtime2.shutdown().unwrap();

    // 3. Third runtime: disabled plugin stays idle until explicitly enabled.
    let (runtime3, mut client3) = Runtime::start_with_preferences(user_pref_path.clone()).unwrap();
    rt.block_on(async {
        let initial_project = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client3.events.recv().await {
                break project;
            }
        };
        client3
            .commands
            .send(Command::StartProject {
                path: None,
                create_new: true,
                options: ProjectStartOptions::default(),
            })
            .await
            .unwrap();
        let p3 = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client3.events.recv().await
                && project.generation > initial_project.generation
            {
                break project;
            }
        };
        loop {
            match client3.events.recv().await.unwrap() {
                Event::InstalledPlugins {
                    project, plugins, ..
                } if project == p3 => {
                    assert_eq!(plugins.len(), 1);
                    assert!(!plugins[0].enabled);
                    break;
                }
                Event::PluginAutoLoaded { .. } => {
                    panic!("disabled persisted plugin must not auto-load")
                }
                _ => {}
            }
        }
        let enable_op = Uuid::new_v4();
        client3
            .commands
            .send(Command::Plugin {
                project: p3,
                operation_id: enable_op,
                action: PluginAction::EnableInstalled {
                    plugin_id: "lab.persist".into(),
                },
            })
            .await
            .unwrap();
        loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::InstalledEnabled { info, .. }),
                ..
            }) = client3.events.recv().await
                && operation_id == enable_op
            {
                assert_eq!(info.manifest.id, "lab.persist");
                break;
            }
        }
    });
    drop(client3);
    runtime3.shutdown().unwrap();

    // 4. Fourth runtime: disable_extensions suppresses auto-load, while Uninstall still deletes files.
    let (runtime4, mut client4) = Runtime::start_with_preferences(user_pref_path).unwrap();
    rt.block_on(async {
        let initial_project = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client4.events.recv().await {
                break project;
            }
        };
        client4
            .commands
            .send(Command::StartProject {
                path: None,
                create_new: true,
                options: ProjectStartOptions {
                    disable_extensions: true,
                    ..ProjectStartOptions::default()
                },
            })
            .await
            .unwrap();
        let p4 = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client4.events.recv().await
                && project.generation > initial_project.generation
            {
                break project;
            }
        };
        loop {
            match client4.events.recv().await.unwrap() {
                Event::InstalledPlugins {
                    project,
                    extensions_disabled,
                    plugins,
                    ..
                } if project == p4 => {
                    assert!(extensions_disabled);
                    assert_eq!(plugins.len(), 1);
                    assert!(plugins[0].enabled);
                    break;
                }
                Event::PluginAutoLoaded { .. } => {
                    panic!("disable_extensions must suppress auto-loading")
                }
                _ => {}
            }
        }
        let enable_op = Uuid::new_v4();
        client4
            .commands
            .send(Command::Plugin {
                project: p4,
                operation_id: enable_op,
                action: PluginAction::EnableInstalled {
                    plugin_id: "lab.persist".into(),
                },
            })
            .await
            .unwrap();
        loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Err(message),
                ..
            }) = client4.events.recv().await
                && operation_id == enable_op
            {
                assert!(message.contains("Disable extensions"), "{message}");
                break;
            }
        }
        let uninstall_op = Uuid::new_v4();
        client4
            .commands
            .send(Command::Plugin {
                project: p4,
                operation_id: uninstall_op,
                action: PluginAction::Uninstall {
                    plugin_id: "lab.persist".into(),
                    instance_id: None,
                },
            })
            .await
            .unwrap();
        loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::Uninstalled { plugin_id, identity }),
                ..
            }) = client4.events.recv().await
                && operation_id == uninstall_op
            {
                assert_eq!(plugin_id, "lab.persist");
                assert!(identity.is_none());
                break;
            }
        }
    });
    drop(client4);
    runtime4.shutdown().unwrap();

    assert!(!plugins_dir.join("packages/lab.persist").exists());
    let store = crate::plugins::PluginRegistryStore::new(Some(plugins_dir));
    assert!(store.list().unwrap().is_empty());
}
