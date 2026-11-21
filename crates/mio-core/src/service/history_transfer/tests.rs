use super::*;
use serde_json::json;

const ORIGINAL: &[u8] = b"POST /original HTTP/1.1\r\nHost: unresolved.invalid\r\nX-Dup: a\r\nX-Dup: b\r\nContent-Length: 3\r\n\r\n\0\xff\x80";
const EDITED: &[u8] = b"PUT /edited HTTP/1.1\r\nHost: edited.invalid\r\nX-Dup: a\r\nX-Dup: b\r\nContent-Length: 3\r\n\r\n\xff\0\x81";

fn fixture() -> (
    State,
    mpsc::Receiver<Event>,
    Vec<ExtMessageSummary>,
    Vec<u8>,
) {
    let (events, receiver) = mpsc::channel(16);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let source = Uuid::new_v4();
    let mut archive = Vec::new();
    let mut items = Vec::new();
    let mut messages = Vec::new();
    for index in 0..2 {
        let item_id = Uuid::new_v4();
        let address = 100 + index;
        let mut fields = json!({});
        for (field_id, role, raw) in [(15, "request", ORIGINAL), (17, "edited_request", EDITED)] {
            archive.extend_from_slice(&[0; 8]);
            let offset = archive.len() as u64;
            archive.extend_from_slice(raw);
            let id = Uuid::new_v4();
            fields[role] = json!({
                "id":field_id,
                "binding":{"source_id":source,"item_id":item_id,"message_id":id},
                "observation":{"state":"value","value":{
                    "kind":"bytes","address":offset-8,"payload_bytes":raw.len(),
                    "element_count":raw.len(),"total_size":raw.len()+8
                }},
                "parsed_request":{"id":2,"observation":{"state":"null"}}
            });
            messages.push(ExtMessageSummary {
                id,
                source_id: source,
                item_id,
                collection: "root.f1.f0".into(),
                item_index: index,
                role: role.into(),
                field_id,
                raw_offset: offset,
                raw_bytes: raw.len() as u64,
                body_offset: Some(raw.len() as u64 - 3),
                declared_version: Some(HttpVersion::Http11),
                method: Some(if field_id == 15 { "POST" } else { "PUT" }.into()),
                request_target: Some(
                    if field_id == 15 {
                        "/original"
                    } else {
                        "/edited"
                    }
                    .into(),
                ),
                status: None,
                diagnostic: None,
            });
        }
        items.push(ExtItem {
            id:item_id,source_id:source,collection:"root.f1.f0".into(),index,
            object_address:Some(address),state:"mapped".into(),
            metadata_json:serde_json::to_vec(&json!({
                "version":1,"context":"http_history_229","slot":{"state":"value","value":address},
                "record":{"object":{"address":address,"object_type":1,"subtype":0},"mapping":{"state":"value","value":fields}}
            })).unwrap(),
        });
    }
    let mut report = ExtImportReport {
        schema_floor: 229,
        schema_revision: 229,
        source_bytes: archive.len() as u64,
        indexed_items: 0,
        indexed_messages: 0,
        collections: vec![],
        repeater_active_tab: None,
        gaps: vec![],
        complete_archive: false,
    };
    state.store.begin_source(source, &report).unwrap();
    state
        .store
        .append_source_chunk(source, 0, &archive)
        .unwrap();
    state.store.put_ext_batch(&items, &messages).unwrap();
    report.indexed_items = items.len() as u64;
    report.indexed_messages = messages.len() as u64;
    report.collections.push(ExtCollectionReport {
        path: "root.f1.f0".into(),
        logical_items: Some(2),
        indexed_items: 2,
        message_count: 4,
        state: "available".into(),
    });
    report.complete_archive = true;
    state.store.finish_source(source, &report).unwrap();
    (state, receiver, messages, archive)
}

fn archived(message: &ExtMessageSummary) -> HistoryTransferEntry {
    HistoryTransferEntry::External {
        source_id: message.source_id,
        item_id: message.item_id,
        message_id: message.id,
    }
}

async fn read(
    state: &mut State,
    receiver: &mut mpsc::Receiver<Event>,
    entries: Vec<HistoryTransferEntry>,
    cancel: CancellationToken,
) -> std::result::Result<Vec<(HistoryEntryId, WorkTab)>, String> {
    let project = state.project;
    let operation_id = Uuid::new_v4();
    let workspace_revision = state.store.workspace().unwrap().revision;
    state
        .command(Command::ReadHistoryTransfer {
            project,
            operation_id,
            workspace_revision,
            entries,
            cancel,
        })
        .await
        .unwrap();
    let Event::HistoryTransferRead {
        project: actual,
        operation_id: operation,
        workspace_revision: revision,
        result,
    } = receiver.try_recv().unwrap()
    else {
        panic!("exactly one correlated proposal event")
    };
    assert_eq!(
        (actual, operation, revision),
        (project, operation_id, workspace_revision)
    );
    assert!(
        receiver.try_recv().is_err(),
        "read proposals cannot emit workspace or network events"
    );
    result
}

#[tokio::test]
async fn mixed_transfer_preserves_order_binary_edits_and_recovery_without_writes_or_sends() {
    let (mut state, mut receiver, messages, archive) = fixture();
    let source = messages[0].source_id;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut original = super::super::tests::completed_flow();
    original.request.url = format!("http://{}/original", listener.local_addr().unwrap());
    state.store.put(&original).unwrap();
    let mut latest = original.clone();
    latest.summary.revision += 1;
    latest.request.url = format!("http://{}/latest", listener.local_addr().unwrap());
    latest.request.body = vec![255, 0, 42];
    state.storage.recovery.push_back(latest.clone().into());
    let entries = vec![
        archived(&messages[2]),
        HistoryTransferEntry::Live {
            flow_id: latest.summary.id,
            revision: latest.summary.revision,
            original: false,
        },
        archived(&messages[1]),
    ];
    let ids: Vec<_> = entries.iter().map(HistoryTransferEntry::id).collect();
    let workspace = state.store.workspace().unwrap();
    state.store.reject_writes(true);
    let proposals = read(&mut state, &mut receiver, entries, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(proposals.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ids);
    assert_eq!(proposals[0].1.draft.raw, ORIGINAL);
    assert_eq!(proposals[1].1.draft.raw, request_to_raw(&latest.request));
    assert_eq!(proposals[2].1.draft.raw, EDITED);
    assert_eq!(proposals[1].1.draft.target, latest.request.url);
    assert!(proposals[0].1.draft.target.is_empty() && proposals[2].1.draft.target.is_empty());
    let tab_ids: HashSet<_> = proposals.iter().map(|(_, tab)| tab.id).collect();
    assert_eq!(tab_ids.len(), 3);
    assert!(!tab_ids.contains(&Uuid::nil()));
    assert!(proposals.iter().all(|(_, tab)| tab.draft.revision == 1
        && tab.last_flow.is_none()
        && tab.archive_response.is_none()
        && tab.history.is_none()
        && tab.group.is_none()));
    assert_eq!(state.store.workspace().unwrap(), workspace);
    assert_eq!(state.storage.recovery.len(), 1);
    assert_eq!(state.store.read(original.summary.id).unwrap(), original);
    assert_eq!(
        state
            .store
            .read_source_range(source, 0, archive.len())
            .unwrap(),
        archive
    );
    assert!(state.repeater.sends.is_empty() && state.repeater.jobs.is_empty() && state.proxy.primary.is_none());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let live_original = read(
        &mut state,
        &mut receiver,
        vec![HistoryTransferEntry::Live {
            flow_id: latest.summary.id,
            revision: latest.summary.revision,
            original: true,
        }],
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        live_original[0].1.draft.raw,
        request_to_raw(&original.request)
    );
    state.store.close().unwrap();
}

#[tokio::test]
async fn any_invalid_transfer_entry_rejects_the_whole_proposal_and_restores_cancel_handler() {
    let (mut state, mut receiver, messages, archive) = fixture();
    let workspace = state.store.workspace().unwrap();
    let live = super::super::tests::completed_flow();
    state.store.put(&live).unwrap();
    let valid = archived(&messages[0]);
    for entries in [
        vec![],
        vec![valid.clone(); 501],
        vec![valid.clone(), valid.clone()],
        vec![
            valid.clone(),
            HistoryTransferEntry::External {
                source_id: messages[2].source_id,
                item_id: messages[2].item_id,
                message_id: messages[0].id,
            },
        ],
        vec![
            valid.clone(),
            HistoryTransferEntry::External {
                source_id: messages[0].source_id,
                item_id: messages[2].item_id,
                message_id: Uuid::new_v4(),
            },
        ],
        vec![
            valid.clone(),
            HistoryTransferEntry::Live {
                flow_id: live.summary.id,
                revision: live.summary.revision + 1,
                original: false,
            },
        ],
        vec![HistoryTransferEntry::Live {
            flow_id: Uuid::nil(),
            revision: 1,
            original: false,
        }],
    ] {
        assert!(
            read(&mut state, &mut receiver, entries, CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(state.store.workspace().unwrap(), workspace);
        assert!(state.repeater.sends.is_empty() && state.repeater.jobs.is_empty());
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        read(&mut state, &mut receiver, vec![valid.clone()], cancel)
            .await
            .unwrap_err()
            .contains("取消")
    );
    let fresh = read(
        &mut state,
        &mut receiver,
        vec![valid],
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(fresh[0].1.draft.raw, ORIGINAL);
    assert_eq!(
        state
            .store
            .read_source_range(messages[0].source_id, 0, archive.len())
            .unwrap(),
        archive
    );
    state.store.close().unwrap();
}

#[tokio::test]
async fn transfer_rejects_old_project_workspace_nil_operation_and_quiescing() {
    let (mut state, mut receiver, messages, _) = fixture();
    let owner = state.project;
    let workspace = state.store.workspace().unwrap();
    for scenario in 0..4 {
        let project = if scenario == 0 {
            ProjectKey {
                generation: owner.generation + 1,
                ..owner
            }
        } else {
            owner
        };
        let operation_id = if scenario == 1 {
            Uuid::nil()
        } else {
            Uuid::new_v4()
        };
        let workspace_revision = workspace.revision + u64::from(scenario == 2);
        state.quiesced = scenario == 3;
        state
            .command(Command::ReadHistoryTransfer {
                project,
                operation_id,
                workspace_revision,
                entries: vec![archived(&messages[0])],
                cancel: CancellationToken::new(),
            })
            .await
            .unwrap();
        assert!(
            matches!(receiver.try_recv().unwrap(),Event::HistoryTransferRead {project:p,operation_id:o,workspace_revision:w,result:Err(_)} if p==project && o==operation_id && w==workspace_revision)
        );
        assert!(receiver.try_recv().is_err());
        assert_eq!(state.store.workspace().unwrap(), workspace);
    }
    state.store.close().unwrap();
}

#[tokio::test]
async fn oversized_archive_declaration_is_rejected_before_allocating_transfer_body() {
    let (mut state, mut receiver, messages, archive) = fixture();
    let mut oversized = messages[0].clone();
    oversized.raw_bytes = MAX_TRANSFER_BYTES as u64 + 1;
    let connection = rusqlite::Connection::open(state.store.test_actual_path()).unwrap();
    let chunks = |connection: &rusqlite::Connection| {
        connection.prepare("SELECT ordinal,raw_bytes,codec,data FROM ext_source_chunks WHERE source=?1 ORDER BY ordinal").unwrap()
            .query_map([oversized.source_id.to_string()], |row| Ok((row.get::<_,u64>(0)?,row.get::<_,u64>(1)?,row.get::<_,u8>(2)?,row.get::<_,Vec<u8>>(3)?)))
            .unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
    };
    let backing = chunks(&connection);
    assert_eq!(
        state
            .store
            .read_source_range(oversized.source_id, 0, archive.len())
            .unwrap(),
        archive
    );
    // Enlarge only untrusted declarations; tiny backing chunks must never be decoded as huge data.
    let declared_bytes = oversized.raw_offset + oversized.raw_bytes;
    let declared_chunks = declared_bytes.div_ceil(1024 * 1024);
    connection
        .execute(
            "UPDATE ext_sources SET source_bytes=?2,written_bytes=?2,next_chunk=?3 WHERE id=?1",
            rusqlite::params![
                oversized.source_id.to_string(),
                declared_bytes,
                declared_chunks
            ],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE ext_messages SET summary=?2 WHERE id=?1",
            rusqlite::params![
                oversized.id.to_string(),
                serde_json::to_vec(&oversized).unwrap()
            ],
        )
        .unwrap();
    let before = state.store.workspace().unwrap();
    let error = read(
        &mut state,
        &mut receiver,
        vec![archived(&oversized)],
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(
        error.contains("64 MiB"),
        "size must be rejected before reading the small source as a huge request: {error}"
    );
    assert_eq!(state.store.workspace().unwrap(), before);
    assert_eq!(chunks(&connection), backing);
    let declared: (u64, u64, u64) = connection
        .query_row(
            "SELECT source_bytes,written_bytes,next_chunk FROM ext_sources WHERE id=?1",
            [oversized.source_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(declared, (declared_bytes, declared_bytes, declared_chunks));
    connection.close().unwrap();
    state.store.close().unwrap();
}

#[tokio::test]
async fn site_map_add_read_and_transfer_preserve_draft_and_archive_identities() {
    let (mut state, mut receiver, messages, _) = fixture();
    let project = state.project;
    let workspace_revision = state.store.workspace().unwrap().revision;
    let draft_owner = Uuid::new_v4();
    let draft_raw = b"POST /v1/submit HTTP/1.1\r\nHost: app.example.com\r\nX-Raw: 1\r\nContent-Length: 4\r\n\r\ndata".to_vec();
    let resp_raw = b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok".to_vec();
    let op_id = Uuid::new_v4();
    state
        .command(Command::AddSiteMapEntry {
            project,
            operation_id: op_id,
            workspace_revision,
            entry: SiteMapEntryInput::Draft {
                owner_id: draft_owner,
                source: TrafficSource::Repeater,
                draft: Draft {
                    revision: 1,
                    target: "https://app.example.com:443".into(),
                    raw: draft_raw.clone(),
                    version: HttpVersion::Http11,
                },
                response_raw: Some(resp_raw.clone()),
            },
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    let receipt = match receiver.try_recv().unwrap() {
        Event::SiteMapEntryAdded {
            project: p,
            operation_id: o,
            workspace_revision: w,
            result: Ok(receipt),
        } if p == project && o == op_id && w == workspace_revision => receipt,
        other => panic!("unexpected event: {other:?}"),
    };
    assert_eq!(receipt.node_key, "https://app.example.com:443/v1/submit");
    assert_eq!(receipt.row.id(), HistoryEntryId::Live(receipt.entry_id));

    state
        .command(Command::ReadSiteMapEntry {
            project,
            selection_revision: 7,
            entry_id: receipt.entry_id,
            revision: receipt.revision,
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    let detail = match receiver.try_recv().unwrap() {
        Event::SiteMapEntryRead {
            project: p,
            selection_revision: 7,
            entry_id: id,
            revision: rev,
            result: Ok(detail),
        } if p == project && id == receipt.entry_id && rev == receipt.revision => detail,
        other => panic!("unexpected read event: {other:?}"),
    };
    assert_eq!(detail.request_raw, draft_raw);
    assert_eq!(detail.response_raw.as_deref(), Some(resp_raw.as_slice()));
    assert_eq!(detail.record.summary.state, FlowState::Pending);

    let transferred = read(
        &mut state,
        &mut receiver,
        vec![HistoryTransferEntry::SiteMap {
            row_id: receipt.row.id(),
            entry_id: receipt.entry_id,
            revision: receipt.revision,
        }],
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(transferred.len(), 1);
    assert_eq!(transferred[0].0, receipt.row.id());
    assert_eq!(transferred[0].1.draft.raw, draft_raw);
    let _ = &messages;
    state.store.close().unwrap();
}
