use std::collections::BTreeMap;

use rusqlite::{
    hooks::{AuthAction, AuthContext, Authorization},
    types::Value,
};

use super::*;
use crate::{ExtCollectionReport, ExtImportReport, ExtMessageSummary, HttpVersion};

const RAW: &[u8] = b"GET /fixture HTTP/1.1\r\n\r\n\0\xff";

fn pending() -> (ProjectStore, Uuid) {
    let mut store = ProjectStore::create(None).unwrap();
    let source_id = Uuid::new_v4();
    store
        .begin_source(
            source_id,
            &ExtImportReport {
                repeater_active_tab: None,
                schema_floor: 228,
                schema_revision: 228,
                source_bytes: RAW.len() as u64,
                indexed_items: 0,
                indexed_messages: 0,
                collections: vec![],
                gaps: vec!["Unverified fields remain in the archive".into()],
                complete_archive: false,
            },
        )
        .unwrap();
    store.append_source_chunk(source_id, 0, RAW).unwrap();
    (store, source_id)
}

fn item(source_id: Uuid, index: u64) -> ExtItem {
    ExtItem {
        id: Uuid::new_v4(),
        source_id,
        collection: "root.f1.f0".into(),
        index,
        object_address: Some(72),
        state: "mapped".into(),
        metadata_json: b"{ \"f20\": {\"state\": \"unverified\"}, \"f17\": null }\n".to_vec(),
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
        raw_offset: 0,
        raw_bytes: RAW.len() as u64,
        body_offset: Some(RAW.len() as u64 - 2),
        declared_version: Some(HttpVersion::Http11),
        method: Some("GET".into()),
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
    for batch in items.chunks(256) {
        store.put_ext_batch(batch, &[]).unwrap();
    }
    for batch in messages.chunks(512) {
        store.put_ext_batch(&[], batch).unwrap();
    }
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

fn summary(item: &ExtItem, message_count: usize) -> ExtItemSummary {
    ExtItemSummary {
        id: item.id,
        source_id: item.source_id,
        collection: item.collection.clone(),
        index: item.index,
        object_address: item.object_address,
        state: item.state.clone(),
        message_count,
    }
}

fn deny_raw(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Read {
            table_name: "ext_source_chunks" | "messages",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

fn deny_details(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Read {
            table_name: "ext_items",
            column_name: "metadata",
            ..
        }
        | AuthAction::Read {
            table_name: "ext_messages",
            column_name: "summary",
            ..
        } => Authorization::Deny,
        _ => deny_raw(context),
    }
}

mod part_gates;
mod part_pages;
