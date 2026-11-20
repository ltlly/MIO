use std::{fs, path::Path};

use rusqlite::{
    OpenFlags,
    hooks::{AuthAction, AuthContext, Authorization},
};
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

fn downgrade_fixture(
    path: &Path,
    schema: i32,
) -> (Uuid, crate::FlowRecord, Option<WebSocketMessage>) {
    let mut store = ProjectStore::create(Some(path)).unwrap();
    let flow = super::super::tests::record();
    store.put(&flow).unwrap();
    let websocket = (schema == 2).then(|| WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id: flow.summary.id,
        sequence: 1,
        direction: WebSocketDirection::ServerToClient,
        kind: WebSocketKind::Binary,
        payload: vec![0, 255],
        observed_unix_ms: 5,
    });
    if let Some(message) = &websocket {
        store.put_websocket(message).unwrap();
    }
    store
        .connection
        .execute_batch(
            "DROP TABLE intruder_results; DROP TABLE intruder_payloads; DROP TABLE intruder_runs;",
        )
        .unwrap();
    store.connection.execute_batch("DROP TABLE ext_messages; DROP TABLE ext_items; DROP TABLE ext_source_chunks; DROP TABLE ext_sources;").unwrap();
    store
        .connection
        .execute_batch(
            "DROP TABLE site_map_entries; DROP INDEX flows_source_started; DROP INDEX flows_source_status; DROP INDEX flows_source_capture; ALTER TABLE flows DROP COLUMN response_length; ALTER TABLE flows DROP COLUMN capture_number; ALTER TABLE flows DROP COLUMN elapsed_ms; ALTER TABLE flows DROP COLUMN response_bytes; ALTER TABLE flows DROP COLUMN status; ALTER TABLE flows DROP COLUMN source; ALTER TABLE flows DROP COLUMN listener_port; DROP TABLE ext_http_highlights; DROP TABLE websocket_originals; DROP TABLE ext_http_notes; DROP TABLE repeater_history; DROP TABLE websocket_notes;",
        )
        .unwrap();
    if schema == 1 {
        store
            .connection
            .execute_batch("DROP TABLE websocket_payloads; DROP TABLE websocket_messages;")
            .unwrap();
    }
    store
        .connection
        .execute_batch("DROP TABLE websocket_connections;")
        .unwrap();
    store
        .connection
        .pragma_update(None, "user_version", schema)
        .unwrap();
    store.checkpoint().unwrap();
    store
        .connection
        .execute_batch("PRAGMA journal_mode=DELETE;")
        .unwrap();
    let id = store.id;
    store.close().unwrap();
    (id, flow, websocket)
}
