use super::*;
use crate::ExtResponseRef;
use serde_json::{Value, json};

const RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\r\nX-Dupe: first\r\nX-Dupe:\tsecond\r\nContent-Length: 4\r\n\r\n\0\xff\r\n";

fn bytes_field(id: u8, offset: u64, length: usize) -> Value {
    json!({"id": id, "observation": {"state": "value", "value": {
        "kind": "bytes", "address": offset - 8, "payload_bytes": length,
        "element_count": length, "total_size": length.max(1) + 8,
    }}})
}

fn pair(raw_response: Option<&[u8]>) -> (ProjectStore, Uuid, ExtItem, ExtMessageSummary) {
    let response_offset = 80 + RAW.len() as u64 + 8;
    let mut archive = vec![0; 80];
    archive.extend_from_slice(RAW);
    archive.extend_from_slice(&[0; 8]);
    archive.extend_from_slice(raw_response.unwrap_or_default());
    if raw_response.is_some_and(|raw| raw.is_empty()) {
        archive.push(0);
    }
    let (mut store, source_id) = pending(&archive);
    let mut item = item(source_id, "root.f2.f1[0].f2", 0);
    let request = message(&item, 2, 80, RAW);
    let mut request_field = bytes_field(2, 80, RAW.len());
    request_field["binding"] =
        json!({"source_id": source_id, "item_id": item.id, "message_id": request.id});
    item.metadata_json = serde_json::to_vec(&json!({
        "version": 1, "context": "repeater_pair_229",
        "slot": {"state": "value", "value": item.object_address},
        "record": {"object": {"object_type": 1, "subtype": 0},
            "mapping": {"state": "value", "value": {
                "request": request_field,
                "response": raw_response.map_or_else(|| json!({"id": 3, "observation": {"state": "null"}}), |raw| bytes_field(3, response_offset, raw.len())),
            }}},
    })).unwrap();
    let mut messages = vec![request.clone()];
    if let Some(raw) = raw_response {
        let mut response = message(&item, 3, response_offset, raw);
        response.role = "response".into();
        response.method = None;
        response.request_target = None;
        response.status = (!raw.is_empty()).then_some(200);
        messages.push(response);
    }
    finish(
        &mut store,
        source_id,
        std::slice::from_ref(&item),
        &messages,
    );
    let mut report = store.ext_report(source_id).unwrap();
    report.schema_floor = 229;
    report.schema_revision = 229;
    store
        .connection
        .execute(
            "UPDATE ext_sources SET report=?1 WHERE id=?2",
            params![serde_json::to_vec(&report).unwrap(), source_id.to_string()],
        )
        .unwrap();
    (store, source_id, item, request)
}

fn load(store: &ProjectStore, source: Uuid, request: &ExtMessageSummary) -> WorkTab {
    store
        .ext_repeater_tab(
            source,
            request.item_id,
            request.id,
            &store.workspace().unwrap(),
            &CancellationToken::new(),
        )
        .unwrap()
}

fn save_tab(store: &mut ProjectStore, tab: WorkTab) -> Workspace {
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    workspace.active = Some(tab.id);
    workspace.tabs.push(tab);
    store.save_workspace(&workspace).unwrap();
    workspace
}

#[test]
fn archived_response_bytes_and_revision_survive_save_as_and_both_reopens_without_flow() {
    let mut response = RESPONSE.to_vec();
    response.extend((0..70_000).map(|index| (index % 256) as u8));
    let (mut store, source, _, request) = pair(Some(&response));
    let tab = load(&store, source, &request);
    let reference = tab.archive_response.clone().unwrap();
    assert_eq!(reference.draft_revision, 1);
    assert_eq!(reference.source_id, source);
    assert_eq!(reference.item_id, request.item_id);
    assert_ne!(reference.message_id, request.id);
    assert_eq!(tab.draft.raw, RAW);
    assert!(tab.last_flow.is_none());
    let probe = store.ext_response_fragment(&reference, 0, 0).unwrap();
    assert!(probe.bytes.is_empty());
    assert_eq!(probe.summary.raw_bytes, response.len() as u64);
    assert_eq!(probe.summary.role, "response");
    let mut read = store
        .ext_response_fragment(&reference, 0, 65536)
        .unwrap()
        .bytes;
    read.extend(
        store
            .ext_response_fragment(&reference, 65536, response.len() - 65536)
            .unwrap()
            .bytes,
    );
    assert_eq!(read, response);
    assert!(store.ext_response_fragment(&reference, 0, 65537).is_err());
    assert!(
        store
            .ext_response_fragment(&reference, u64::MAX, 1)
            .is_err()
    );
    let mut workspace = save_tab(&mut store, tab);
    workspace.tabs[0].draft.raw.extend_from_slice(b"edited");
    workspace.tabs[0].draft.revision += 1;
    workspace.closed.push(workspace.tabs.remove(0));
    workspace.active = None;
    workspace.revision += 1;
    let reads = deny_chunks(&store);
    store.save_workspace(&workspace).unwrap();
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "workspace persistence must not read response bodies"
    );
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("response.mio");
    let copy = directory.path().join("response-copy.mio");
    store.save(Some(&first)).unwrap();
    store.save(None).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&first, &copy] {
        let reopened = ProjectStore::open(path).unwrap();
        assert_eq!(reopened.workspace().unwrap(), workspace);
        assert_eq!(
            reopened
                .ext_response_fragment(&reference, 0, RESPONSE.len())
                .unwrap()
                .bytes,
            RESPONSE
        );
        assert_eq!(reopened.page(0, 1, "").unwrap().0, 0);
        assert_eq!(
            reopened.workspace().unwrap().closed[0]
                .archive_response
                .as_ref()
                .unwrap()
                .draft_revision,
            1
        );
        reopened.close().unwrap();
    }
}

#[test]
fn archived_empty_response_is_distinct_from_null_and_unmapped_response() {
    for raw in [None, Some(&[][..])] {
        let (store, source, item, request) = pair(raw);
        let tab = load(&store, source, &request);
        assert_eq!(tab.archive_response.is_some(), raw.is_some());
        if let Some(reference) = &tab.archive_response {
            assert!(
                store
                    .ext_response_fragment(reference, 0, 0)
                    .unwrap()
                    .bytes
                    .is_empty()
            );
        }
        for state in ["missing", "null", "failed", "unavailable"] {
            let mut metadata: Value = serde_json::from_slice(&item.metadata_json).unwrap();
            metadata["record"]["mapping"]["value"]["response"]["observation"] =
                json!({"state": state});
            store
                .connection
                .execute(
                    "UPDATE ext_items SET metadata=?1 WHERE id=?2",
                    params![serde_json::to_vec(&metadata).unwrap(), item.id.to_string()],
                )
                .unwrap();
            assert!(load(&store, source, &request).archive_response.is_none());
        }
    }
}

#[test]
fn archived_response_value_without_indexed_response_fails_instead_of_becoming_none() {
    let (store, source, _, request) = pair(Some(RESPONSE));
    let reference = load(&store, source, &request).archive_response.unwrap();
    store
        .connection
        .execute(
            "DELETE FROM ext_messages WHERE id=?1",
            [reference.message_id.to_string()],
        )
        .unwrap();
    assert!(
        store
            .ext_repeater_tab(
                source,
                request.item_id,
                request.id,
                &store.workspace().unwrap(),
                &CancellationToken::new()
            )
            .is_err()
    );
    assert!(store.ext_response_fragment(&reference, 0, 0).is_err());
}

#[test]
fn saved_archive_response_cannot_rebind_revision_but_can_yield_to_a_stored_flow() {
    let (mut store, source, _, request) = pair(Some(RESPONSE));
    let tab = load(&store, source, &request);
    let saved = save_tab(&mut store, tab);
    let mut proposal = saved.clone();
    proposal.revision += 1;
    proposal.tabs[0].draft.revision += 1;
    proposal.tabs[0].draft.raw.extend_from_slice(b"edited");
    let edited_revision = proposal.tabs[0].draft.revision;
    proposal.tabs[0]
        .archive_response
        .as_mut()
        .unwrap()
        .draft_revision = edited_revision;
    assert!(
        store.save_workspace(&proposal).is_err(),
        "an old archive response must not claim the newly edited draft revision"
    );
    assert_eq!(store.workspace().unwrap(), saved);

    let mut flow = crate::store::tests::record();
    flow.summary.source = crate::TrafficSource::Repeater;
    store.put(&flow).unwrap();
    proposal.tabs[0].archive_response = None;
    proposal.tabs[0].last_flow = Some(flow.summary.id);
    store.save_workspace(&proposal).unwrap();
    assert_eq!(store.workspace().unwrap(), proposal);
    assert_eq!(store.page(0, 1, "").unwrap().0, 1);
}

#[test]
fn archived_response_binding_rejects_foreign_ids_metadata_changes_and_invalid_workspace_revision() {
    let (mut store, source, item, request) = pair(Some(RESPONSE));
    let tab = load(&store, source, &request);
    let reference = tab.archive_response.clone().unwrap();
    for forged in [
        ExtResponseRef {
            source_id: Uuid::new_v4(),
            ..reference.clone()
        },
        ExtResponseRef {
            item_id: Uuid::new_v4(),
            ..reference.clone()
        },
        ExtResponseRef {
            message_id: request.id,
            ..reference.clone()
        },
        ExtResponseRef {
            draft_revision: 0,
            ..reference.clone()
        },
    ] {
        assert!(store.ext_response_fragment(&forged, 0, 0).is_err());
    }
    let mut workspace = save_tab(&mut store, tab);
    let saved = workspace.clone();
    workspace.revision += 1;
    workspace.tabs[0].last_flow = Some(Uuid::new_v4());
    assert!(store.save_workspace(&workspace).is_err());
    workspace.tabs[0].last_flow = None;
    workspace.tabs[0]
        .archive_response
        .as_mut()
        .unwrap()
        .draft_revision = 2;
    assert!(store.save_workspace(&workspace).is_err());
    assert_eq!(store.workspace().unwrap(), saved);
    let original: Value = serde_json::from_slice(&item.metadata_json).unwrap();
    for (pointer, forged) in [
        ("/context", json!("repeater_pair")),
        ("/slot/value", json!(0)),
        ("/record/object/subtype", json!(1)),
        (
            "/record/mapping/value/request/binding/message_id",
            json!(reference.message_id),
        ),
        (
            "/record/mapping/value/request/observation/value/address",
            json!(0),
        ),
        ("/record/mapping/value/response/id", json!(2)),
        (
            "/record/mapping/value/response/observation/value/kind",
            json!("utf16"),
        ),
        (
            "/record/mapping/value/response/observation/value/address",
            json!(0),
        ),
        (
            "/record/mapping/value/response/observation/value/payload_bytes",
            json!(0),
        ),
        (
            "/record/mapping/value/response/observation/value/element_count",
            json!(0),
        ),
        (
            "/record/mapping/value/response/observation/value/total_size",
            json!(0),
        ),
    ] {
        let mut metadata = original.clone();
        *metadata.pointer_mut(pointer).unwrap() = forged;
        store
            .connection
            .execute(
                "UPDATE ext_items SET metadata=?1 WHERE id=?2",
                params![serde_json::to_vec(&metadata).unwrap(), item.id.to_string()],
            )
            .unwrap();
        assert!(
            store.ext_response_fragment(&reference, 0, 0).is_err(),
            "{pointer}"
        );
        assert!(store.workspace().is_err(), "{pointer}");
        let mut proposal = saved.clone();
        proposal.revision += 1;
        assert!(store.save_workspace(&proposal).is_err(), "{pointer}");
    }
}

#[test]
fn archived_response_dangling_reference_is_rejected_before_open_writes() {
    let (mut store, source, _, request) = pair(Some(RESPONSE));
    let tab = load(&store, source, &request);
    let mut workspace = save_tab(&mut store, tab);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dangling-response.mio");
    store.save(Some(&path)).unwrap();
    store.close().unwrap();
    workspace.tabs[0]
        .archive_response
        .as_mut()
        .unwrap()
        .message_id = Uuid::new_v4();
    let edit = rusqlite::Connection::open(&path).unwrap();
    edit.execute(
        "UPDATE metadata SET value=?1 WHERE key='workspace'",
        [serde_json::to_vec(&workspace).unwrap()],
    )
    .unwrap();
    edit.close().unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(ProjectStore::open(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn workspace_save_updates_revision_and_preserves_unrelated_extension_table() {
    let mut store = ProjectStore::create(None).unwrap();
    let before = store.workspace().unwrap();
    store
        .connection
        .execute_batch(
            "CREATE TABLE unrelated_extension(id INTEGER PRIMARY KEY, payload BLOB NOT NULL);
             INSERT INTO unrelated_extension VALUES(1, X'F00D');",
        )
        .unwrap();
    let mut proposal = before.clone();
    proposal.revision += 1;
    store.save_workspace(&proposal).unwrap();
    assert_eq!(store.workspace().unwrap(), proposal);
    let retained: Vec<u8> = store
        .connection
        .query_row(
            "SELECT payload FROM unrelated_extension WHERE id=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, [0xF0, 0x0D]);
}
