use super::*;
use crate::{
    ExtCollectionReport, ExtImportReport, HistoryRow, WebSocketDirection, WebSocketKind,
    WebSocketMessage, WebSocketNotesChange,
};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use serde_json::json;

fn project(store: &ProjectStore) -> ProjectKey {
    ProjectKey {
        id: store.id,
        generation: 1,
    }
}

fn fixture(text: &str) -> (ProjectStore, Uuid, Vec<ExtItem>) {
    fixture_with_http_items(text, 2)
}

pub(super) fn fixture_with_http_items(
    text: &str,
    http_items: u64,
) -> (ProjectStore, Uuid, Vec<ExtItem>) {
    assert!((2..=500).contains(&http_items));
    let mut store = ProjectStore::create(None).unwrap();
    let source = Uuid::new_v4();
    let archive = vec![0_u8; 4096];
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
    store.begin_source(source, &report).unwrap();
    store.append_source_chunk(source, 0, &archive).unwrap();
    let mut items = Vec::new();
    let slots = [
        ("root.f1.f0", 0),
        ("root.f1.f0", 1),
        ("root.f1.f1", 0),
        ("root.f1.f1", 1),
        ("root.f1.f1", 2),
        ("root.f1.f2", 0),
    ]
    .into_iter()
    .chain((2..http_items).map(|index| ("root.f1.f0", index)));
    for (collection, index) in slots {
        let id = Uuid::new_v4();
        let count = text.encode_utf16().count();
        let field = |id, value| json!({"id":id,"observation":{"state":"value","value":value}});
        let decoded = |address, value| json!({"object":{"address":address,"object_type":0,"subtype":0},"mapping":{"state":"value","value":value}});
        let node = decoded(
            256,
            json!({
                "backing":field(1,json!({"kind":"utf16","address":512,"element_count":count,"payload_bytes":count*2,"total_size":(count*2).max(1)+8})),
                "length":field(2,json!(count)), "text":{"state":"value","value":text},
            }),
        );
        let mut annotations = field(27, decoded(128, json!({"note":field(2,node)})));
        annotations["binding"] = json!({"source_id":source,"item_id":id});
        let metadata = json!({
            "version":1,"context":"http_history_229","slot":{"state":"value","value":100},
            "record":{"object":{"address":100,"object_type":1,"subtype":0},"mapping":{"state":"value","value":{"annotations":annotations}}},
        });
        items.push(ExtItem {
            id,
            source_id: source,
            collection: collection.into(),
            index,
            object_address: Some(100),
            state: "mapped".into(),
            metadata_json: serde_json::to_vec(&metadata).unwrap(),
        });
    }
    store.put_ext_batch(&items, &[]).unwrap();
    for (collection, count) in [
        ("root.f1.f0", http_items),
        ("root.f1.f1", 3),
        ("root.f1.f2", 1),
    ] {
        report.collections.push(ExtCollectionReport {
            path: collection.into(),
            logical_items: Some(count),
            indexed_items: count,
            message_count: 0,
            state: "available".into(),
        });
    }
    report.indexed_items = items.len() as u64;
    report.complete_archive = true;
    store.finish_source(source, &report).unwrap();
    (store, source, items)
}

fn replace_metadata(store: &ProjectStore, item: &ExtItem, metadata: &Value) {
    store
        .connection
        .execute(
            "UPDATE ext_items SET metadata=?2 WHERE id=?1",
            params![item.id.to_string(), serde_json::to_vec(metadata).unwrap()],
        )
        .unwrap();
}

mod part_atomic;
mod part_save_as;
mod part_source;

fn websocket_message(flow: Uuid) -> WebSocketMessage {
    WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id: flow,
        sequence: 1,
        direction: WebSocketDirection::ClientToServer,
        kind: WebSocketKind::Binary,
        payload: vec![0, 255, 42],
        observed_unix_ms: 5,
    }
}
