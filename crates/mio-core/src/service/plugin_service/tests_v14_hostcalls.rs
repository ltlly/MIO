use std::{collections::HashMap, fs, sync::Arc};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use mio_plugin_api::protocol::{Envelope, HostCall, envelope, host_call};

use super::*;
use crate::plugins::{
    NativeTrust, PermissionBroker, ServiceOperation, ServiceRequest, ServiceResult, VerifiedPackage,
};

struct V14Fixture {
    state: State,
    identity: ChannelIdentity,
    plugin_id: String,
    _dir: tempfile::TempDir,
}

fn setup_v14_fixture(grants: Vec<Permission>) -> V14Fixture {
    let dir = tempfile::tempdir().unwrap();
    let worker_path = dir.path().join("v14-worker");
    fs::write(&worker_path, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&worker_path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        dir.path().join("manifest.json"),
        br#"{"id":"lab.v14-test","version":"1.0.0","api_version":1,"engine":"native","entry":"v14-worker","permissions":["traffic.read","traffic.modify","draft.write","repeater.open","network.send"],"contributions":["suite_panel"]}"#,
    )
    .unwrap();
    let package = VerifiedPackage::read(dir.path()).unwrap();
    let trust = NativeTrust::confirmed(&package, grants.clone()).unwrap();
    let (events, _rx) = mpsc::channel(256);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: state.project,
    };
    let initial = Arc::new(PanelSnapshot {
        revision: 1,
        title: "v1.4".into(),
        root: Some(mio_plugin_api::panel::Node::new(
            1,
            "root",
            mio_plugin_api::panel::node::Kind::Text(mio_plugin_api::panel::Text {
                text: "ok".into(),
                tone: 0,
            }),
        )),
    });
    let (worker, _queue) =
        crate::plugins::test_supervisor(identity, package.manifest.clone(), initial.clone());
    let (_tx, services) = mpsc::channel(16);
    let info = LoadedPlugin {
        identity,
        directory: package.directory.clone(),
        manifest: package.manifest.clone(),
        grants,
        tab_id: None,
    };
    let manifest_modified = package.manifest_modified;
    let manifest_size = package.manifest_bytes.len() as u64;
    let entry_modified = package.modified;
    let entry_size = package.executable_size;
    state.plugins.instances.insert(
        identity.instance_id,
        Instance {
            worker,
            services,
            package,
            manifest_modified,
            manifest_size,
            entry_modified,
            entry_size,
            auto_reload: false,
            trust,
            info,
            panel: initial,
            source: None,
            resources: HashMap::new(),
        },
    );
    V14Fixture {
        state,
        identity,
        plugin_id: "lab.v14-test".into(),
        _dir: dir,
    }
}

async fn call_service(
    f: &mut V14Fixture,
    request_id: u64,
    operation: ServiceOperation,
) -> std::result::Result<ServiceResult, Failure> {
    let (reply, rx) = oneshot::channel();
    f.state
        .plugin_service(ServiceRequest {
            identity: f.identity,
            plugin_id: f.plugin_id.clone(),
            panel_id: protocol::PANEL_ID,
            request_id,
            deadline: tokio::time::Instant::now() + Duration::from_secs(5),
            operation,
            cancel: CancellationToken::new(),
            reply,
        })
        .await;
    rx.await.unwrap()
}

#[tokio::test]
async fn v14_scope_sitemap_annotations_organiser_audit_and_collaborator_work_end_to_end() {
    let mut f = setup_v14_fixture(vec![
        Permission::TrafficRead,
        Permission::TrafficModify,
        Permission::DraftWrite,
        Permission::RepeaterOpen,
        Permission::NetworkSend,
    ]);

    // 1. Scope check + update
    let res = call_service(
        &mut f,
        1,
        ServiceOperation::ScopeCheck {
            url: "https://api.example.com/v1/users".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(res, ServiceResult::ScopeStatus { in_scope: false }));

    call_service(
        &mut f,
        2,
        ServiceOperation::ScopeUpdate {
            url_prefix: "https://example.com/".into(),
            exclude: false,
            include_subdomains: true,
        },
    )
    .await
    .unwrap();
    let res = call_service(
        &mut f,
        3,
        ServiceOperation::ScopeCheck {
            url: "https://api.example.com/v1/users".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(res, ServiceResult::ScopeStatus { in_scope: true }));

    call_service(
        &mut f,
        4,
        ServiceOperation::ScopeUpdate {
            url_prefix: "https://api.example.com/admin".into(),
            exclude: true,
            include_subdomains: false,
        },
    )
    .await
    .unwrap();
    let res = call_service(
        &mut f,
        5,
        ServiceOperation::ScopeCheck {
            url: "https://api.example.com/admin/panel".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(res, ServiceResult::ScopeStatus { in_scope: false }));

    // 2. Persist a live flow -> Annotations & SiteMap
    let flow_id = Uuid::new_v4();
    let record = FlowRecord {
        summary: FlowSummary {
            id: flow_id,
            revision: 1,
            source: TrafficSource::Proxy,
            method: "GET".into(),
            url: "https://api.example.com/v1/users?id=7".into(),
            status: Some(200),
            state: FlowState::Complete,
            request_bytes: 0,
            response_bytes: 2,
            started_unix_ms: 1_700_000_000_000,
            elapsed_ms: 12,
        },
        request: HttpRequest {
            method: "GET".into(),
            url: "https://api.example.com/v1/users?id=7".into(),
            version: HttpVersion::Http11,
            headers: vec![Header {
                name: "Host".into(),
                value: b"api.example.com".to_vec(),
            }],
            body: Vec::new(),
            trailers: Vec::new(),
        },
        response: Some(HttpResponse {
            status: 200,
            version: HttpVersion::Http11,
            reason_phrase: Some(b"OK".to_vec()),
            headers: vec![Header {
                name: "Content-Length".into(),
                value: b"2".to_vec(),
            }],
            body: b"ok".to_vec(),
            trailers: Vec::new(),
        }),
    };
    f.state.persist(&record).unwrap();

    let updated_ann = call_service(
        &mut f,
        6,
        ServiceOperation::UpdateAnnotations {
            flow_id,
            notes: Some("idor target".into()),
            highlight_color: Some(2),
        },
    )
    .await
    .unwrap();
    let ServiceResult::Annotations {
        notes,
        highlight_color,
        notes_revision,
        highlight_revision,
        ..
    } = updated_ann
    else {
        panic!("expected Annotations");
    };
    assert_eq!(notes, "idor target");
    assert_eq!(highlight_color, 2);
    assert!(notes_revision >= 1 && highlight_revision >= 1);

    let read_ann = call_service(&mut f, 7, ServiceOperation::ReadAnnotations { flow_id })
        .await
        .unwrap();
    assert!(matches!(
        read_ann,
        ServiceResult::Annotations {
            ref notes,
            highlight_color: 2,
            ..
        } if notes == "idor target"
    ));

    let sitemap = call_service(
        &mut f,
        8,
        ServiceOperation::QuerySiteMap {
            prefix: String::new(),
            in_scope_only: true,
            offset: 0,
            limit: 10,
        },
    )
    .await
    .unwrap();
    let ServiceResult::SiteMap { total, rows, .. } = sitemap else {
        panic!("expected SiteMap");
    };
    assert_eq!(total, 1);
    assert_eq!(rows[0].url, "https://api.example.com/v1/users?id=7");
    assert_eq!(rows[0].notes_preview, "idor target");
    assert_eq!(rows[0].highlight_color, 2);

    // 3. OpenOrganiser
    let org_res = call_service(
        &mut f,
        9,
        ServiceOperation::OpenOrganiser {
            request: record.request.clone(),
            response: record.response.clone(),
            notes: "triage from plugin".into(),
            highlight_color: Some(4),
        },
    )
    .await
    .unwrap();
    assert!(matches!(org_res, ServiceResult::Resource { .. }));
    let saved_ws = f.state.store.workspace().unwrap();
    assert_eq!(saved_ws.organiser_items.len(), 1);
    assert_eq!(saved_ws.organiser_items[0].notes, "triage from plugin");
    assert_eq!(saved_ws.organiser_items[0].highlight, Some(4));

    // 4. SubmitAuditIssue
    call_service(
        &mut f,
        10,
        ServiceOperation::SubmitAuditIssue {
            flow_id: Some(flow_id),
            url: record.request.url.clone(),
            finding: protocol::PluginFindingDto {
                title: "Standalone Plugin Issue".into(),
                detail: "Found by v1.4 plugin".into(),
                remediation: "Validate authorization".into(),
                severity: "High".into(),
                confidence: "Certain".into(),
                parameter: "id".into(),
                evidence: "ok".into(),
            },
        },
    )
    .await
    .unwrap();
    let audit = f.state.store.read_audit_state().unwrap();
    assert!(
        audit
            .findings
            .iter()
            .any(|item| item.title == "Standalone Plugin Issue"
                && item.issue_id == "0x00900102")
    );

    // 5. CollaboratorGenerate + real HTTP callback + CollaboratorPoll
    let gen_res = call_service(
        &mut f,
        11,
        ServiceOperation::CollaboratorGenerate {
            label: "v14-ssrf".into(),
        },
    )
    .await
    .unwrap();
    let ServiceResult::CollaboratorPayload(token) = gen_res else {
        panic!("expected CollaboratorPayload");
    };
    assert!(!token.token.is_empty());
    let parsed = url::Url::parse(&token.http_callback_url).unwrap();
    let addr = format!(
        "{}:{}",
        parsed.host_str().unwrap(),
        parsed.port_or_known_default().unwrap()
    );
    let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let req_bytes = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        parsed.path(),
        addr
    );
    stream.write_all(req_bytes.as_bytes()).await.unwrap();
    let mut buf = [0u8; 256];
    let _ = stream.read(&mut buf).await;

    let mut matched = Vec::new();
    for attempt in 0..20 {
        let poll_res = call_service(
            &mut f,
            20 + attempt,
            ServiceOperation::CollaboratorPoll {
                token_filter: token.token.clone(),
                limit: 16,
            },
        )
        .await
        .unwrap();
        let ServiceResult::CollaboratorInteractions(items) = poll_res else {
            panic!("expected CollaboratorInteractions");
        };
        if !items.is_empty() {
            matched = items;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(matched.len(), 1, "must observe HTTP Collaborator callback");
    assert_eq!(matched[0].token.as_deref(), Some(token.token.as_str()));

    f.state.collaborator.stop_project(f.state.project).await.unwrap();
}

#[tokio::test]
async fn v14_broker_and_host_enforce_permissions() {
    let mut f = setup_v14_fixture(vec![Permission::TrafficRead]);
    let req = HttpRequest {
        method: "GET".into(),
        url: "https://example.com/".into(),
        version: HttpVersion::Http11,
        headers: vec![],
        body: vec![],
        trailers: vec![],
    };
    for (idx, op) in [
        ServiceOperation::ScopeUpdate {
            url_prefix: "https://example.com/".into(),
            exclude: false,
            include_subdomains: false,
        },
        ServiceOperation::UpdateAnnotations {
            flow_id: Uuid::new_v4(),
            notes: Some("x".into()),
            highlight_color: None,
        },
        ServiceOperation::OpenOrganiser {
            request: req.clone(),
            response: None,
            notes: String::new(),
            highlight_color: None,
        },
        ServiceOperation::CollaboratorGenerate {
            label: "x".into(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        let err = call_service(&mut f, (idx + 1) as u64, op).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Denied as i32);
    }

    let broker = PermissionBroker::new(f.identity, [Permission::TrafficRead]);
    let env = Envelope::new(
        f.identity,
        1,
        1,
        envelope::Body::Ready(mio_plugin_api::panel::Empty {}),
        Duration::from_secs(5),
    );
    let call = HostCall {
        operation: Some(host_call::Operation::CollaboratorGenerate(
            protocol::CollaboratorGenerateDto {
                label: "nope".into(),
            },
        )),
        parent_request_id: 0,
    };
    assert_eq!(
        broker.authorize(&env, call).unwrap_err().code,
        ErrorCode::Denied as i32
    );
}
