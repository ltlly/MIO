use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::tests::{deliver_query, drain, handshake, message};
use super::*;
use crate::{ExtCollectionReport, ExtImportReport, ExtItem, ExtMessageSummary};

const ARCHIVE_CHUNK: &[u8] =
    b"HDR00000HDR10000raw-client-binary\0\xffHDR20000edited-client-frame!HDR30000server-pong-text";
const RAW_ADDR: u64 = 8;
const RAW_BYTES: &[u8] = b"raw-client-binary\0\xff";
const EDITED_ADDR: u64 = 35;
const EDITED_BYTES: &[u8] = b"edited-client-frame!";
const SERVER_ADDR: u64 = 63;
const SERVER_BYTES: &[u8] = b"server-pong-text";

fn array_obs(address: u64, len: usize) -> Value {
    json!({
        "kind": "bytes",
        "address": address,
        "payload_bytes": len,
        "element_count": len,
        "total_size": len.max(1) + 8,
    })
}

fn msg_summary(
    item: &ExtItem,
    field_id: u8,
    role: &str,
    addr: u64,
    len: usize,
    url: &str,
) -> ExtMessageSummary {
    ExtMessageSummary {
        id: Uuid::new_v4(),
        source_id: item.source_id,
        item_id: item.id,
        collection: item.collection.clone(),
        item_index: item.index,
        role: role.into(),
        field_id,
        raw_offset: addr + 8,
        raw_bytes: len as u64,
        body_offset: Some(0),
        declared_version: None,
        method: None,
        request_target: Some(url.to_owned()),
        status: None,
        diagnostic: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_ext_ws_item(
    source_id: Uuid,
    collection: &str,
    index: u64,
    object_address: u64,
    direction: WebSocketDirection,
    kind: WebSocketKind,
    raw: (u64, &[u8]),
    edited: Option<(u64, &[u8])>,
    note: Option<&str>,
) -> (ExtItem, Vec<ExtMessageSummary>) {
    let mut item = ExtItem {
        id: Uuid::new_v4(),
        source_id,
        collection: collection.into(),
        index,
        object_address: Some(object_address),
        state: "mapped".into(),
        metadata_json: Vec::new(),
    };
    let url = "wss://ws.example.test/live";
    let payload_msg = msg_summary(&item, 5, "websocket_payload", raw.0, raw.1.len(), url);
    let mut messages = vec![payload_msg.clone()];
    let edited_field = if let Some((ed_addr, ed_bytes)) = edited {
        let ed_msg = msg_summary(
            &item,
            6,
            "edited_websocket_payload",
            ed_addr,
            ed_bytes.len(),
            url,
        );
        messages.push(ed_msg.clone());
        json!({
            "id": 6,
            "observation": {"state": "value", "value": array_obs(ed_addr, ed_bytes.len())},
            "binding": {"source_id": source_id, "item_id": item.id, "message_id": ed_msg.id},
        })
    } else {
        json!({
            "id": 6,
            "observation": {"state": "null"},
            "binding": {"source_id": source_id, "item_id": item.id, "message_id": Value::Null},
        })
    };
    let annotations = match note {
        Some(text) => json!({
            "id": 27,
            "observation": {"state": "value", "value": {"mapping": {"state": "value", "value": {"note": {"observation": {"state": "value", "value": {"mapping": {"state": "value", "value": {"text": {"state": "value", "value": text}}}}}}}}}}
        }),
        None => json!({"id": 27, "observation": {"state": "null"}}),
    };
    let metadata = json!({
        "version": 1,
        "context": "websocket_history",
        "slot": {"state": "value", "value": object_address},
        "record": {
            "object": {
                "address": object_address,
                "object_type": 1,
                "subtype": 1,
                "descriptors": ([1, 2, 3, 4, 5, 6, 11, 13, 27].map(|id| json!({"id": id}))),
            },
            "mapping": {
                "state": "value",
                "value": {
                    "connection_id": {"id": 1, "observation": {"state": "value", "value": 7}},
                    "upgrade": {
                        "id": 2,
                        "observation": {"state": "value", "value": {"mapping": {"state": "value", "value": {"url": {"state": "value", "value": url}}}}}
                    },
                    "direction": {"id": 3, "observation": {"state": "value", "value": direction}},
                    "kind": {"id": 4, "observation": {"state": "value", "value": kind}},
                    "payload": {
                        "id": 5,
                        "observation": {"state": "value", "value": array_obs(raw.0, raw.1.len())},
                        "binding": {"source_id": source_id, "item_id": item.id, "message_id": payload_msg.id},
                    },
                    "edited_payload": edited_field,
                    "time": {"id": 11, "observation": {"state": "value", "value": 1_710_000_500_000_i64}},
                    "highlight": {"id": 13, "observation": {"state": "value", "value": 0}},
                    "annotations": annotations,
                    "unmapped_field_ids": [],
                }
            }
        }
    });
    item.metadata_json = serde_json::to_vec(&metadata).unwrap();
    (item, messages)
}

fn seed_ext_ws_archive(store: &mut ProjectStore) -> (Uuid, Uuid) {
    let source_id = Uuid::new_v4();
    let initial = ExtImportReport {
        repeater_active_tab: None,
        schema_floor: 229,
        schema_revision: 229,
        source_bytes: ARCHIVE_CHUNK.len() as u64,
        indexed_items: 0,
        indexed_messages: 0,
        collections: vec![],
        gaps: vec![],
        complete_archive: false,
    };
    store.begin_source(source_id, &initial).unwrap();
    store
        .append_source_chunk(source_id, 0, ARCHIVE_CHUNK)
        .unwrap();
    let (first_item, first_msgs) = build_ext_ws_item(
        source_id,
        "root.f1.f2",
        0,
        128,
        WebSocketDirection::ClientToServer,
        WebSocketKind::Binary,
        (RAW_ADDR, RAW_BYTES),
        Some((EDITED_ADDR, EDITED_BYTES)),
        Some("archived-ws-note"),
    );
    let (alias_item, alias_msgs) = build_ext_ws_item(
        source_id,
        "root.f1.f3",
        0,
        128,
        WebSocketDirection::ClientToServer,
        WebSocketKind::Binary,
        (RAW_ADDR, RAW_BYTES),
        Some((EDITED_ADDR, EDITED_BYTES)),
        Some("archived-ws-note"),
    );
    let (second_item, second_msgs) = build_ext_ws_item(
        source_id,
        "root.f1.f2",
        1,
        256,
        WebSocketDirection::ServerToClient,
        WebSocketKind::Text,
        (SERVER_ADDR, SERVER_BYTES),
        None,
        None,
    );
    let items = vec![first_item.clone(), alias_item, second_item.clone()];
    let mut messages = Vec::new();
    messages.extend(first_msgs);
    messages.extend(alias_msgs);
    messages.extend(second_msgs);
    store.put_ext_batch(&items, &messages).unwrap();

    let mut collections = BTreeMap::<String, ExtCollectionReport>::new();
    for item in &items {
        let entry = collections
            .entry(item.collection.clone())
            .or_insert_with(|| ExtCollectionReport {
                path: item.collection.clone(),
                logical_items: Some(0),
                indexed_items: 0,
                message_count: 0,
                state: "available".into(),
            });
        entry.indexed_items += 1;
        entry.logical_items = Some(entry.logical_items.unwrap().max(item.index + 1));
    }
    for msg in &messages {
        collections.get_mut(&msg.collection).unwrap().message_count += 1;
    }
    let mut report = store.ext_report(source_id).unwrap();
    report.indexed_items = items.len() as u64;
    report.indexed_messages = messages.len() as u64;
    report.collections = collections.into_values().collect();
    report.complete_archive = true;
    store.finish_source(source_id, &report).unwrap();
    (first_item.id, second_item.id)
}

#[tokio::test]
async fn ext_websocket_archive_merges_into_history_queries_detail_reads_and_notes() {
    let (events, mut receiver) = mpsc::channel(32);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let flow = handshake();
    let live = message(flow.summary.id, 1);
    state.store.put(&flow).unwrap();
    state.store.put_websocket(&live).unwrap();
    let (archived_edited_id, archived_plain_id) = seed_ext_ws_archive(&mut state.store);

    state
        .command(Command::QueryWebSocketHistory {
            project: state.project,
            query_revision: 1,
            flow_id: None,
            selected_message_id: Some(archived_edited_id),
            filter: WebSocketHistoryFilter::default(),
            offset: 0,
            limit: 10,
        })
        .await
        .unwrap();
    deliver_query(&mut state).await;
    let Event::WebSocketHistoryPage {
        total,
        rows,
        selected_match,
        ..
    } = drain(&mut receiver).pop().unwrap()
    else {
        panic!("expected WebSocketHistoryPage");
    };
    assert_eq!(total, 3);
    assert_eq!(selected_match, Some((archived_edited_id, true)));
    assert_eq!(
        rows.iter().map(|r| (r.id, r.ordinal)).collect::<Vec<_>>(),
        vec![
            (live.id, 1),
            (archived_edited_id, 2),
            (archived_plain_id, 3)
        ]
    );
    assert!(rows[1].edited);
    assert_eq!(rows[1].notes_preview, "archived-ws-note");
    assert_eq!(rows[1].url, "wss://ws.example.test/live");
    assert!(!rows[2].edited);

    // Filter by only_with_notes and original payload search term.
    state
        .command(Command::QueryWebSocketHistory {
            project: state.project,
            query_revision: 2,
            flow_id: None,
            selected_message_id: Some(archived_plain_id),
            filter: WebSocketHistoryFilter {
                only_with_notes: true,
                term: "raw-client-binary".into(),
                ..WebSocketHistoryFilter::default()
            },
            offset: 0,
            limit: 10,
        })
        .await
        .unwrap();
    deliver_query(&mut state).await;
    let Event::WebSocketHistoryPage {
        total,
        rows,
        selected_match,
        ..
    } = drain(&mut receiver).pop().unwrap()
    else {
        panic!("expected filtered WebSocketHistoryPage");
    };
    assert_eq!(total, 1);
    assert_eq!(rows[0].id, archived_edited_id);
    assert_eq!(selected_match, Some((archived_plain_id, false)));

    // ReadWebSocket returns edited payload as record.payload and original payload as Some(RAW_BYTES).
    state
        .command(Command::ReadWebSocket {
            project: state.project,
            selection_revision: 5,
            message_id: archived_edited_id,
        })
        .await
        .unwrap();
    let Event::WebSocketDetail {
        record,
        original_payload,
        ..
    } = drain(&mut receiver).pop().unwrap()
    else {
        panic!("expected WebSocketDetail for archived item");
    };
    assert_eq!(record.id, archived_edited_id);
    assert_eq!(record.flow_id, Uuid::nil());
    assert_eq!(record.payload, EDITED_BYTES);
    assert_eq!(original_payload.as_deref(), Some(RAW_BYTES));

    // ReadWebSocketNotes returns the archived External note text.
    state
        .command(Command::ReadWebSocketNotes {
            project: state.project,
            selection_revision: 6,
            message_id: archived_edited_id,
        })
        .await
        .unwrap();
    assert!(matches!(
        drain(&mut receiver).pop().unwrap(),
        Event::WebSocketNotes {
            result: Ok(WebSocketNotes { revision: 0, ref text }),
            ..
        } if text == "archived-ws-note"
    ));
}
