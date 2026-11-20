use super::*;
use crate::{ExtCollectionReport, ExtHttpNotesChange, ExtImportReport, HistoryRow};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use serde_json::json;

fn project(store: &ProjectStore) -> ProjectKey {
    ProjectKey {
        id: store.id,
        generation: 1,
    }
}

fn fixture(color: u8) -> (ProjectStore, Uuid, Vec<ExtItem>) {
    let mut store = ProjectStore::create(None).unwrap();
    let source = Uuid::new_v4();
    let mut report = ExtImportReport {
        schema_floor: 229,
        schema_revision: 229,
        source_bytes: 1024,
        indexed_items: 0,
        indexed_messages: 0,
        collections: vec![],
        repeater_active_tab: None,
        gaps: vec![],
        complete_archive: false,
    };
    store.begin_source(source, &report).unwrap();
    store.append_source_chunk(source, 0, &[0; 1024]).unwrap();
    let mut items = Vec::new();
    for (collection, index) in [
        ("root.f1.f0", 0),
        ("root.f1.f0", 1),
        ("root.f1.f1", 0),
        ("root.f1.f1", 1),
        ("root.f1.f1", 2),
        ("root.f1.f2", 0),
    ] {
        let id = Uuid::new_v4();
        let binding = json!({"source_id":source,"item_id":id});
        let metadata = json!({"version":1,"context":"http_history_229","slot":{"state":"value","value":100},
        "record":{"object":{"address":100,"object_type":1,"subtype":0},"mapping":{"state":"value","value":{
            "highlight":{"id":13,"binding":binding,"observation":{"state":"value","value":color}},
            "annotations":{"id":27,"binding":binding,"observation":{"state":"null"}}
        }}}});
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
    for (collection, count) in [("root.f1.f0", 2), ("root.f1.f1", 3), ("root.f1.f2", 1)] {
        report.collections.push(ExtCollectionReport {
            path: collection.into(),
            logical_items: Some(count),
            indexed_items: count,
            message_count: 0,
            state: "available".into(),
        });
    }
    report.indexed_items = 6;
    report.complete_archive = true;
    store.finish_source(source, &report).unwrap();
    (store, source, items)
}

fn change(source: Uuid, item: Uuid, revision: u64, color: u8) -> ExtHttpHighlightChange {
    ExtHttpHighlightChange {
        source_id: source,
        item_id: item,
        expected_highlight_revision: revision,
        color,
    }
}

fn metadata(store: &ProjectStore, item: &ExtItem, edit: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_slice(&item.metadata_json).unwrap();
    edit(&mut value);
    store
        .connection
        .execute(
            "UPDATE ext_items SET metadata=?2 WHERE id=?1",
            params![item.id.to_string(), serde_json::to_vec(&value).unwrap()],
        )
        .unwrap();
}

mod part_aliases;
mod part_conflicts;
mod part_save_as;

fn websocket_message(flow: Uuid) -> crate::WebSocketMessage {
    crate::WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id: flow,
        sequence: 1,
        direction: crate::WebSocketDirection::ClientToServer,
        kind: crate::WebSocketKind::Binary,
        payload: vec![0, 255, 42],
        observed_unix_ms: 5,
    }
}
