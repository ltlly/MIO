use super::{tests_state::*, *};

#[test]
fn explicit_resync_restores_equal_revision_native_input_without_weakening_normal_edits() {
    let mut lab = Lab::new();
    Lab::set_editor(&mut lab.raw, b"late worker transform".to_vec()).unwrap();
    let mut authoritative = lab.raw.clone();
    authoritative.contents = b"latest native input".to_vec();
    let snapshot = PanelSnapshot {
        revision: 2,
        title: "test".into(),
        root: Some(Node::new(
            RAW_NODE,
            "Raw",
            node::Kind::Editor(authoritative.clone()),
        )),
    };
    assert!(
        lab.resync(&snapshot).is_ok(),
        "explicit host Resync must resolve equal-revision conflicts in favor of native input"
    );
    assert_eq!(lab.raw, authoritative);
    assert!(
        lab.sync_editor(&EditorSnapshot {
            document_id: RAW_DOCUMENT,
            revision: authoritative.revision,
            contents: b"ordinary conflicting event".to_vec(),
            ..Default::default()
        })
        .is_err(),
        "normal editor events remain revision/content checked"
    );
}

#[test]
fn explicit_resync_adopts_authoritative_viewport_and_split() {
    let mut lab = Lab::new();
    let mut host = PanelState::new(lab.snapshot()).unwrap();
    host.observe_event(&UiEvent {
        node_id: TABLE_NODE,
        event: Some(ui_event::Event::Viewport(ViewportRequest {
            revision: 7,
            offset: 0,
            limit: 64,
        })),
        editors: vec![],
    })
    .unwrap();
    host.observe_event(&UiEvent {
        node_id: 3,
        event: Some(ui_event::Event::SplitResized(0.65)),
        editors: vec![],
    })
    .unwrap();
    lab.resync(host.snapshot()).unwrap();
    assert_eq!(lab.viewport_revision, 7);
    assert_eq!(lab.split_fraction, 0.65);
}

#[test]
fn hooks_session_payload_and_kv_intruder_buttons_behave_as_specified() {
    let mut lab = Lab::new();
    lab.grants = vec![
        Permission::TrafficRead,
        Permission::RepeaterOpen,
        Permission::TrafficModify,
        Permission::SessionHook,
        Permission::PayloadProcess,
        Permission::Storage,
    ];
    let mut ctx = client(&lab);
    lab.snapshot().validate().unwrap();

    let mut drop_req: RequestDto = request().into();
    drop_req.headers.push(HeaderDto {
        name: "X-Mio-Lab-Action".into(),
        value: b"drop".to_vec(),
    });
    let res = lab
        .http_hook(HttpHookCall {
            flow_id: Uuid::new_v4().as_bytes().to_vec(),
            source: SourceDto::Proxy as i32,
            plugin_id: String::new(),
            response_stage: false,
            response_head_only: false,
            request: Some(drop_req),
            response: None,
        })
        .unwrap();
    assert_eq!(res.action, HookAction::Drop as i32);

    lab.event(
        &mut ctx,
        UiEvent {
            node_id: TOGGLE_HOOKS,
            event: Some(ui_event::Event::Click(Empty {})),
            editors: vec![],
        },
    )
    .unwrap();
    assert!(lab.auto_tag_hooks);

    let modified = lab
        .http_hook(HttpHookCall {
            flow_id: Uuid::new_v4().as_bytes().to_vec(),
            source: SourceDto::Proxy as i32,
            plugin_id: String::new(),
            response_stage: false,
            response_head_only: false,
            request: Some(request().into()),
            response: None,
        })
        .unwrap();
    assert_eq!(modified.action, HookAction::Modify as i32);
    assert!(
        modified
            .request
            .unwrap()
            .headers
            .iter()
            .any(|h| h.name == "x-protocol-lab" && h.value == b"hooked")
    );

    let ws = lab.ws_hook(WebSocketHookCall {
        flow_id: Uuid::new_v4().as_bytes().to_vec(),
        message_id: Uuid::new_v4().as_bytes().to_vec(),
        url: "ws://127.0.0.1/".into(),
        client_to_server: true,
        is_binary: false,
        payload: b"hello".to_vec(),
    });
    assert_eq!(ws.action, HookAction::Modify as i32);
    assert_eq!(ws.payload, b"[lab] hello");

    let sess = lab
        .session_action(SessionActionCall {
            source: SourceDto::Repeater as i32,
            request: Some(request().into()),
        })
        .unwrap();
    assert!(sess.modified);

    let generated = lab
        .payload(PayloadCall {
            op: PayloadOp::Generate as i32,
            config: "a, b\nc".into(),
            inputs: vec![],
            max_items: 2,
        })
        .unwrap();
    assert_eq!(generated.outputs, vec![b"a".to_vec(), b"b".to_vec()]);

    let processed = lab
        .payload(PayloadCall {
            op: PayloadOp::Process as i32,
            config: String::new(),
            inputs: vec![b"one".to_vec()],
            max_items: 10,
        })
        .unwrap();
    assert_eq!(processed.outputs, vec![b"lab:one".to_vec()]);

    Lab::set_editor(&mut lab.raw, vec![b'x'; 5000]).unwrap();
    let editors = vec![
        EditorSnapshot {
            document_id: RAW_DOCUMENT,
            revision: lab.raw.revision,
            contents: lab.raw.contents.clone(),
            ..Default::default()
        },
        EditorSnapshot {
            document_id: DECODED_DOCUMENT,
            revision: lab.decoded.revision,
            contents: lab.decoded.contents.clone(),
            ..Default::default()
        },
    ];
    let (op, _) = lab
        .click(
            &mut ctx,
            &UiEvent {
                node_id: SAVE_KV,
                event: Some(ui_event::Event::Click(Empty {})),
                editors,
            },
        )
        .unwrap()
        .unwrap();
    assert!(
        matches!(op, host_call::Operation::StorageSet(entry) if entry.key == "scratch" && entry.value.len() == 4096)
    );

    lab.pending.insert(99, PendingService::LoadKv);
    lab.host_result(
        &mut ctx,
        99,
        HostResult {
            outcome: Some(host_result::Outcome::StorageValue(StorageValue {
                present: true,
                value: b"from-kv".to_vec(),
            })),
        },
    )
    .unwrap();
    assert_eq!(lab.decoded.contents, b"from-kv");
}

#[test]
fn context_menu_preferences_and_response_editor_behave_as_specified() {
    let mut lab = Lab::new();
    lab.grants = vec![
        Permission::TrafficRead,
        Permission::DraftWrite,
        Permission::RepeaterOpen,
        Permission::Storage,
    ];
    let mut ctx = client(&lab);
    let menu_res = lab
        .context_menu(ContextMenuCall {
            source: SourceDto::Repeater as i32,
            action_id: "default".into(),
            selection_start: 0,
            selection_end: 0,
            selected_bytes: vec![],
            request: Some(request().into()),
            response: None,
        })
        .unwrap();
    assert!(menu_res.log_line.contains("lab.protocol 上下文菜单"));
    assert!(menu_res.status_message.contains("Repeater"));
    let proposed = menu_res.open_repeater.expect("open_repeater proposal");
    assert!(
        proposed
            .headers
            .iter()
            .any(|h| h.name == "x-protocol-lab-context" && h.value == b"menu-v1")
    );
    assert!(menu_res.replace_request.is_none());

    lab.event(
        &mut ctx,
        UiEvent {
            node_id: AUTO_HOOK_CHECKBOX,
            event: Some(ui_event::Event::Toggled(true)),
            editors: vec![],
        },
    )
    .unwrap();
    assert!(lab.auto_tag_hooks);
    lab.event(
        &mut ctx,
        UiEvent {
            node_id: CODEC_SELECT,
            event: Some(ui_event::Event::SelectedOption(1)),
            editors: vec![],
        },
    )
    .unwrap();
    assert!(lab.url_safe_base64);

    let replace_res = lab
        .context_menu(ContextMenuCall {
            source: SourceDto::Repeater as i32,
            action_id: "replace".into(),
            selection_start: 0,
            selection_end: 3,
            selected_bytes: vec![0xfb, 0xff],
            request: Some(request().into()),
            response: None,
        })
        .unwrap();
    let replaced_raw =
        String::from_utf8(replace_res.replace_request.expect("replace_request")).unwrap();
    assert!(replaced_raw.contains("x-protocol-lab-context: menu-v1"));
    assert!(replaced_raw.ends_with("-_8="));

    Lab::set_editor(&mut lab.raw, b"pref-value".to_vec()).unwrap();
    let editors = vec![
        EditorSnapshot {
            document_id: RAW_DOCUMENT,
            revision: lab.raw.revision,
            contents: lab.raw.contents.clone(),
            ..Default::default()
        },
        EditorSnapshot {
            document_id: DECODED_DOCUMENT,
            revision: lab.decoded.revision,
            contents: lab.decoded.contents.clone(),
            ..Default::default()
        },
    ];
    let (pref_set_op, _) = lab
        .click(
            &mut ctx,
            &UiEvent {
                node_id: SAVE_PREF,
                event: Some(ui_event::Event::Click(Empty {})),
                editors,
            },
        )
        .unwrap()
        .unwrap();
    assert!(
        matches!(pref_set_op, host_call::Operation::PreferencesSet(entry) if entry.key == "scratch_pref" && entry.value == b"pref-value")
    );

    let (pref_get_op, _) = lab
        .click(
            &mut ctx,
            &UiEvent {
                node_id: LOAD_PREF,
                event: Some(ui_event::Event::Click(Empty {})),
                editors: vec![],
            },
        )
        .unwrap()
        .unwrap();
    assert!(
        matches!(pref_get_op, host_call::Operation::PreferencesGet(key) if key == "scratch_pref")
    );
    lab.pending.insert(105, PendingService::LoadPref);
    lab.host_result(
        &mut ctx,
        105,
        HostResult {
            outcome: Some(host_result::Outcome::StorageValue(StorageValue {
                present: true,
                value: b"from-pref".to_vec(),
            })),
        },
    )
    .unwrap();
    assert_eq!(lab.decoded.contents, b"from-pref");

    let resp_id = Uuid::new_v4();
    let resp = HttpResponse {
        status: 200,
        version: mio_plugin_api::HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![],
        body: b"5Z+657q/".to_vec(),
        trailers: vec![],
    };
    lab.load_response(ResponseContext {
        resource: resp_id,
        revision: 3,
        read_only: false,
        response: resp.clone(),
    })
    .unwrap();
    let committed = lab
        .commit_response(CommitResponseEditor {
            resource_id: resp_id.as_bytes().to_vec(),
            message_revision: 3,
            response: Some(resp.into()),
            editor: Some(EditorSnapshot {
                document_id: DECODED_DOCUMENT,
                revision: lab.decoded.revision + 1,
                contents: "响应编辑".as_bytes().to_vec(),
                ..Default::default()
            }),
        })
        .unwrap();
    assert_eq!(committed.document_id, DECODED_DOCUMENT);
    assert!(committed.response.is_some());
}

#[test]
fn passive_check_detects_audit_header_and_body_markers() {
    let lab = Lab::new();
    let clean_resp = ResponseDto {
        status: 200,
        version: HttpVersionDto::Http11 as i32,
        headers: vec![HeaderDto {
            name: "Content-Type".into(),
            value: b"text/plain".to_vec(),
        }],
        body: b"normal response".to_vec(),
        trailers: vec![],
    };
    let clean = lab
        .passive_check(PassiveCheckCall {
            flow_id: Uuid::new_v4().as_bytes().to_vec(),
            url: "http://127.0.0.1/clean".into(),
            request: Some(request().into()),
            response: Some(clean_resp.clone()),
        })
        .unwrap();
    assert!(clean.findings.is_empty());

    let mut header_resp = clean_resp.clone();
    header_resp.headers.push(HeaderDto {
        name: "X-Plugin-Audit-Flag".into(),
        value: b"enabled".to_vec(),
    });
    let by_header = lab
        .passive_check(PassiveCheckCall {
            flow_id: Uuid::new_v4().as_bytes().to_vec(),
            url: "http://127.0.0.1/hdr".into(),
            request: Some(request().into()),
            response: Some(header_resp),
        })
        .unwrap();
    assert_eq!(by_header.findings.len(), 1);
    assert_eq!(by_header.findings[0].parameter, "X-Plugin-Audit-Flag");

    let mut body_resp = clean_resp;
    body_resp.body = b"prefix PLUGIN_VULN_MARKER suffix".to_vec();
    let by_body = lab
        .passive_check(PassiveCheckCall {
            flow_id: Uuid::new_v4().as_bytes().to_vec(),
            url: "http://127.0.0.1/body".into(),
            request: Some(request().into()),
            response: Some(body_resp),
        })
        .unwrap();
    assert_eq!(by_body.findings.len(), 1);
    assert_eq!(by_body.findings[0].evidence, "PLUGIN_VULN_MARKER");
}
