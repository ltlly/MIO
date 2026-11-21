use std::fs;

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{
    ExtCollectionReport, HttpVersion, WebSocketDirection, WebSocketKind, WebSocketMessage,
};

fn initial(source_bytes: usize) -> ExtImportReport {
    ExtImportReport {
        repeater_active_tab: None,
        schema_floor: 228,
        schema_revision: 228,
        source_bytes: source_bytes as u64,
        indexed_items: 0,
        indexed_messages: 0,
        collections: vec![],
        gaps: vec!["Unknown fields remain in the complete source; no plugin is executed".into()],
        complete_archive: false,
    }
}

mod part_archive;
mod part_batches;
mod part_gates;
mod part_limits;
mod part_migration;
mod part_ranges;

fn item(source_id: Uuid, index: u64) -> ExtItem {
    ExtItem {
        id: Uuid::new_v4(),
        source_id,
        collection: "root.f1.f0".into(),
        index,
        object_address: Some(72),
        state: "mapped".into(),
        metadata_json: br#"{"fields":[15,18],"unknown":{"field":20,"state":"unverified"}}"#
            .to_vec(),
    }
}

fn message(item: &ExtItem, field_id: u8) -> ExtMessageSummary {
    ExtMessageSummary {
        id: Uuid::new_v4(),
        source_id: item.source_id,
        item_id: item.id,
        collection: item.collection.clone(),
        item_index: item.index,
        role: "request".into(),
        field_id,
        raw_offset: 3,
        raw_bytes: 20,
        body_offset: Some(18),
        declared_version: Some(HttpVersion::Http11),
        method: Some("GET".into()),
        request_target: Some("/unresolved-target".into()),
        status: None,
        diagnostic: Some("No connection destination was inferred".into()),
    }
}

fn final_report(
    source_bytes: usize,
    items: &[ExtItem],
    messages: &[ExtMessageSummary],
) -> ExtImportReport {
    let mut report = initial(source_bytes);
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
    report.collections = collections.into_values().collect();
    report.indexed_items = items.len() as u64;
    report.indexed_messages = messages.len() as u64;
    report.complete_archive = true;
    report
}

fn archived(raw: &[u8]) -> (ProjectStore, Uuid) {
    let mut store = ProjectStore::create(None).unwrap();
    let id = Uuid::new_v4();
    store.begin_source(id, &initial(raw.len())).unwrap();
    let chunks: Vec<_> = raw.chunks(EXT_CHUNK_BYTES).map(<[u8]>::to_vec).collect();
    store.append_source_chunks(id, 0, &chunks).unwrap();
    (store, id)
}

fn deny_archive_reads(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Read {
            table_name: "ext_source_chunks" | "messages",
            ..
        }
        | AuthAction::Read {
            table_name: "ext_items",
            column_name: "metadata",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}
