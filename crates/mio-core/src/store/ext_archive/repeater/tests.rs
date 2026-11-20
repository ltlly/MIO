use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use rusqlite::{
    hooks::{AuthAction, AuthContext, Authorization},
    params,
};

use super::*;
use crate::{ExtCollectionReport, ExtImportReport, ExtItem, ExtMessageSummary, HttpVersion};

const RAW: &[u8] = b"POST /fixture HTTP/1.1\r\nHost: unresolved.invalid:443\r\nX-A: \xff\r\nX-A:\tsecond\r\nContent-Length: 4\r\n\r\n\0\xff\r\n";

#[path = "tests/groups.rs"]
mod group_tests;
#[path = "tests/history_target.rs"]
mod history_target;
#[path = "tests/response.rs"]
mod response_tests;

mod part_limits;
mod part_selection;
mod part_titles;

fn pending(raw: &[u8]) -> (ProjectStore, Uuid) {
    let mut store = ProjectStore::create(None).unwrap();
    let source_id = Uuid::new_v4();
    store
        .begin_source(
            source_id,
            &ExtImportReport {
                repeater_active_tab: None,
                schema_floor: 228,
                schema_revision: 228,
                source_bytes: raw.len() as u64,
                indexed_items: 0,
                indexed_messages: 0,
                collections: vec![],
                gaps: vec!["Connection service is unverified".into()],
                complete_archive: false,
            },
        )
        .unwrap();
    for (ordinal, chunk) in raw.chunks(EXT_CHUNK_BYTES).enumerate() {
        store
            .append_source_chunk(source_id, ordinal as u64, chunk)
            .unwrap();
    }
    (store, source_id)
}

fn item(source_id: Uuid, collection: &str, index: u64) -> ExtItem {
    ExtItem {
        id: Uuid::new_v4(),
        source_id,
        collection: collection.into(),
        index,
        object_address: Some(72),
        state: "mapped".into(),
        metadata_json: b"{}".to_vec(),
    }
}

fn message(item: &ExtItem, field_id: u8, offset: u64, raw: &[u8]) -> ExtMessageSummary {
    ExtMessageSummary {
        id: Uuid::new_v4(),
        source_id: item.source_id,
        item_id: item.id,
        collection: item.collection.clone(),
        item_index: item.index,
        role: "request".into(),
        field_id,
        raw_offset: offset,
        raw_bytes: raw.len() as u64,
        body_offset: raw
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .map(|offset| offset as u64 + 4),
        declared_version: Some(HttpVersion::Http11),
        method: Some("POST".into()),
        request_target: Some("/fixture".into()),
        status: None,
        diagnostic: None,
    }
}

fn finish(
    store: &mut ProjectStore,
    source_id: Uuid,
    items: &[ExtItem],
    messages: &[ExtMessageSummary],
) {
    store.put_ext_batch(items, messages).unwrap();
    let mut collections = BTreeMap::<String, ExtCollectionReport>::new();
    for item in items {
        let collection = collections
            .entry(item.collection.clone())
            .or_insert_with(|| ExtCollectionReport {
                path: item.collection.clone(),
                logical_items: Some(0),
                indexed_items: 0,
                message_count: 0,
                state: "available; fields not fully mapped".into(),
            });
        collection.indexed_items += 1;
        collection.logical_items = Some(collection.logical_items.unwrap().max(item.index + 1));
    }
    for message in messages {
        collections
            .get_mut(&message.collection)
            .unwrap()
            .message_count += 1;
    }
    let mut report = store.ext_report(source_id).unwrap();
    report.indexed_items = items.len() as u64;
    report.indexed_messages = messages.len() as u64;
    report.collections = collections.into_values().collect();
    report.complete_archive = true;
    store.finish_source(source_id, &report).unwrap();
}

fn one() -> (ProjectStore, Uuid, ExtItem, ExtMessageSummary) {
    let (mut store, source_id) = pending(RAW);
    let item = item(source_id, "root.f1.f0", 0);
    let message = message(&item, 15, 0, RAW);
    finish(
        &mut store,
        source_id,
        std::slice::from_ref(&item),
        std::slice::from_ref(&message),
    );
    (store, source_id, item, message)
}

fn deny_chunks(store: &ProjectStore) -> Arc<AtomicUsize> {
    let reads = Arc::new(AtomicUsize::new(0));
    let observed = reads.clone();
    store
        .connection
        .authorizer(Some(move |context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "ext_source_chunks",
                ..
            } => {
                observed.fetch_add(1, Ordering::SeqCst);
                Authorization::Deny
            }
            _ => Authorization::Allow,
        }));
    reads
}

fn replace_summary(store: &ProjectStore, message: &ExtMessageSummary) {
    store
        .connection
        .execute(
            "UPDATE ext_messages SET summary=?2 WHERE id=?1",
            params![message.id.to_string(), serde_json::to_vec(message).unwrap()],
        )
        .unwrap();
}

fn intruder_draft(tab: WorkTab) -> crate::IntruderTab {
    crate::IntruderTab {
        id: tab.id,
        title: tab.title,
        group: None,
        spec: crate::IntruderSpec {
            resource_pool: None,
            baseline_request: false,
            revision: 1,
            draft: tab.draft,
            attack_type: crate::intruder::AttackType::Sniper,
            positions: vec![],
            payload_sets: vec![],
            max_concurrent_requests: 0,
            request_timeout_ms: 0,
            update_host_header: false,
            save_to_project: true,
        },
        last_run: None,
    }
}

fn titled_229() -> (ProjectStore, Uuid, ExtItem, ExtMessageSummary) {
    let archive = [vec![0; 80], RAW.to_vec()].concat();
    let (mut store, source_id) = pending(&archive);
    let mut item = item(source_id, "root.f2.f1", 0);
    let mut message = message(&item, 4, 80, RAW);
    message.role = "current_request".into();
    item.metadata_json = serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "context": "repeater_current_229",
        "slot": {"state": "value", "value": item.object_address},
        "record": {
            "object": {"object_type": 0, "subtype": 2},
            "mapping": {"state": "value", "value": {"current_request": {
                "id": 4,
                "observation": {"state": "value", "value": {
                    "kind": "bytes", "address": 72, "payload_bytes": RAW.len(),
                    "element_count": RAW.len(), "total_size": RAW.len() + 8,
                }},
                "binding": {"source_id": source_id, "item_id": item.id, "message_id": message.id},
                "title": {"id": 0, "observation": {"state": "value", "value": {
                    "object": {"object_type": 0, "subtype": 0},
                    "mapping": {"state": "value", "value": "mio-title-汉🙂"},
                }}},
            }}},
        },
    }))
    .unwrap();
    finish(
        &mut store,
        source_id,
        std::slice::from_ref(&item),
        std::slice::from_ref(&message),
    );
    let mut report = store.ext_report(source_id).unwrap();
    report.schema_floor = 229;
    report.schema_revision = 229;
    report.gaps =
        vec!["Synthetic partial mapping; unverified titles and tool data stay archived".into()];
    store
        .connection
        .execute(
            "UPDATE ext_sources SET report=?1",
            [serde_json::to_vec(&report).unwrap()],
        )
        .unwrap();
    (store, source_id, item, message)
}
