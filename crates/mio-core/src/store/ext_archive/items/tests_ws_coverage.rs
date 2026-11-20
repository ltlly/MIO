use std::collections::BTreeMap;

use mio_plugin_api::{WebSocketDirection, WebSocketKind};
use serde_json::{Value, json};

use super::*;
use crate::store::ext_archive::EXT_FRAGMENT_BYTES;
use crate::{
    ExtCategory, ExtCollectionReport, ExtImportReport, ExtMessageSummary, ScopeRule,
    inspect_mio_ext_archive, read_mio_ext_websocket_fragment,
};

const ARCHIVE: &[u8] =
    b"HDR00000HDR10000client-hello-frame\0\xffHDR20000edited-frame-bytes!padding-tail-bytes";
const PAYLOAD_ARRAY_ADDR: u64 = 8;
const PAYLOAD_BYTES: &[u8] = b"client-hello-frame\0\xff";
const EDITED_ARRAY_ADDR: u64 = 36;
const EDITED_BYTES: &[u8] = b"edited-frame-bytes!";

fn begin_229(gaps: Vec<String>) -> (ProjectStore, Uuid) {
    let mut store = ProjectStore::create(None).unwrap();
    let source_id = Uuid::new_v4();
    let initial = ExtImportReport {
        repeater_active_tab: None,
        schema_floor: 229,
        schema_revision: 229,
        source_bytes: ARCHIVE.len() as u64,
        indexed_items: 0,
        indexed_messages: 0,
        collections: vec![],
        gaps,
        complete_archive: false,
    };
    store.begin_source(source_id, &initial).unwrap();
    store.append_source_chunk(source_id, 0, ARCHIVE).unwrap();
    (store, source_id)
}

fn finish_source(
    store: &mut ProjectStore,
    source_id: Uuid,
    items: &[ExtItem],
    messages: &[ExtMessageSummary],
) {
    store.put_ext_batch(items, messages).unwrap();
    let mut collections = BTreeMap::<String, ExtCollectionReport>::new();
    for item in items {
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

fn ws_message(
    item: &ExtItem,
    field_id: u8,
    role: &str,
    array_address: u64,
    payload_len: usize,
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
        raw_offset: array_address + 8,
        raw_bytes: payload_len as u64,
        body_offset: Some(0),
        declared_version: None,
        method: None,
        request_target: Some(url.to_owned()),
        status: None,
        diagnostic: None,
    }
}

fn array_observation(address: u64, payload_len: usize) -> Value {
    json!({
        "kind": "bytes",
        "address": address,
        "payload_bytes": payload_len,
        "element_count": payload_len,
        "total_size": payload_len.max(1) + 8,
    })
}

fn unmapped_item(
    source_id: Uuid,
    collection: &str,
    index: u64,
    addr: u64,
    tag: (u8, u8),
    field_id: u8,
) -> ExtItem {
    ExtItem {
        id: Uuid::new_v4(),
        source_id,
        collection: collection.into(),
        index,
        object_address: Some(addr),
        state: "unmapped".into(),
        metadata_json: serde_json::to_vec(&json!({
            "context": "unmapped_collection",
            "record": {"object": {"address": addr, "object_type": tag.0, "subtype": tag.1, "descriptors": [{"id": field_id}]}}
        }))
        .unwrap(),
    }
}

fn mapped_ws_entry(
    source_id: Uuid,
    collection: &str,
    index: u64,
    object_address: u64,
    with_edited: bool,
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
    let url = "wss://ws.example.test/stream";
    let payload_msg = ws_message(
        &item,
        5,
        "websocket_payload",
        PAYLOAD_ARRAY_ADDR,
        PAYLOAD_BYTES.len(),
        url,
    );
    let mut messages = vec![payload_msg.clone()];
    let edited_msg = with_edited.then(|| {
        let msg = ws_message(
            &item,
            6,
            "edited_websocket_payload",
            EDITED_ARRAY_ADDR,
            EDITED_BYTES.len(),
            url,
        );
        messages.push(msg.clone());
        msg
    });
    let edited_field = if let Some(msg) = &edited_msg {
        json!({
            "id": 6,
            "observation": {"state": "value", "value": array_observation(EDITED_ARRAY_ADDR, EDITED_BYTES.len())},
            "binding": {"source_id": source_id, "item_id": item.id, "message_id": msg.id},
        })
    } else {
        json!({
            "id": 6,
            "observation": {"state": "null"},
            "binding": {"source_id": source_id, "item_id": item.id, "message_id": Value::Null},
        })
    };
    let descriptors = [1, 2, 3, 4, 5, 6, 11, 13, 27, 99].map(|id| json!({"id": id}));
    let metadata = json!({
        "version": 1,
        "context": "websocket_history",
        "slot": {"state": "value", "value": object_address},
        "record": {
            "object": {
                "address": object_address,
                "object_type": 1,
                "subtype": 1,
                "descriptors": descriptors,
            },
            "mapping": {
                "state": "value",
                "value": {
                    "connection_id": {"id": 1, "observation": {"state": "value", "value": 42}},
                    "upgrade": {
                        "id": 2,
                        "observation": {"state": "value", "value": {"mapping": {"state": "value", "value": {"url": {"state": "value", "value": url}}}}}
                    },
                    "direction": {"id": 3, "observation": {"state": "value", "value": WebSocketDirection::ClientToServer}},
                    "kind": {"id": 4, "observation": {"state": "value", "value": WebSocketKind::Binary}},
                    "payload": {
                        "id": 5,
                        "observation": {"state": "value", "value": array_observation(PAYLOAD_ARRAY_ADDR, PAYLOAD_BYTES.len())},
                        "binding": {"source_id": source_id, "item_id": item.id, "message_id": payload_msg.id},
                    },
                    "edited_payload": edited_field,
                    "time": {"id": 11, "observation": {"state": "value", "value": 1_710_000_123_000_i64}},
                    "highlight": {"id": 13, "observation": {"state": "value", "value": 3}},
                    "annotations": {
                        "id": 27,
                        "observation": {"state": "value", "value": {"mapping": {"state": "value", "value": {"note": {"observation": {"state": "value", "value": {"mapping": {"state": "value", "value": {"text": {"state": "value", "value": "ws-audit-note"}}}}}}}}}}
                    },
                    "unmapped_field_ids": [99],
                }
            }
        }
    });
    item.metadata_json = serde_json::to_vec(&metadata).unwrap();
    (item, messages)
}

#[test]
fn coverage_report_and_public_archive_inspect_aggregate_categories_and_dedupe_ws_aliases() {
    let gaps = vec!["root.f4.f0 (Embedded Project Config): 已提取 Scope 规则。".into()];
    let (mut store, source_id) = begin_229(gaps);
    let http_item = ExtItem {
        id: Uuid::new_v4(),
        source_id,
        collection: "root.f1.f0".into(),
        index: 0,
        object_address: Some(64),
        state: "mapped".into(),
        metadata_json: serde_json::to_vec(&json!({
            "record": {"object": {"object_type": 1, "subtype": 0, "descriptors": [{"id": 15}, {"id": 18}, {"id": 19}]}}
        }))
        .unwrap(),
    };
    let (ws_primary, mut messages) = mapped_ws_entry(source_id, "root.f1.f2", 0, 128, true);
    let (ws_alias, alias_messages) = mapped_ws_entry(source_id, "root.f1.f3", 0, 128, false);
    messages.extend(alias_messages);
    let (ws_secondary_only, sec_messages) = mapped_ws_entry(source_id, "root.f1.f3", 1, 256, false);
    messages.extend(sec_messages);
    let ws_unmapped = unmapped_item(source_id, "root.f1.f2", 1, 384, (1, 7), 50);
    let extra_unmapped = unmapped_item(source_id, "root.f9.f0", 0, 512, (4, 2), 8);

    let items = [
        http_item,
        ws_primary.clone(),
        ws_alias,
        ws_secondary_only.clone(),
        ws_unmapped.clone(),
        extra_unmapped,
    ];
    finish_source(&mut store, source_id, &items, &messages);

    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    workspace.scope.include.push(ScopeRule {
        id: Uuid::new_v4(),
        enabled: true,
        prefix: "https://ws.example.test".into(),
        include_subdomains: false,
    });
    store.save_workspace(&workspace).unwrap();

    let coverage = store.ext_coverage_report(source_id).unwrap();
    assert_eq!(coverage.categories.len(), 6);
    let http_cov = coverage.category(ExtCategory::ProxyHttpHistory).unwrap();
    assert_eq!((http_cov.recovered_items, http_cov.unmapped_slots), (1, 0));
    assert!(http_cov.mapped_fields.iter().any(|f| f == "f15"));
    assert!(http_cov.unmapped_fields.iter().any(|f| f == "f19"));

    let site_cov = coverage
        .category(ExtCategory::TargetSiteMapAndScope)
        .unwrap();
    assert_eq!(site_cov.recovered_items, 2);
    assert_eq!(site_cov.state, "mapped");

    let ws_cov = coverage
        .category(ExtCategory::ProxyWebSocketHistory)
        .unwrap();
    assert_eq!((ws_cov.recovered_items, ws_cov.unmapped_slots), (3, 1));
    assert_eq!(ws_cov.state, "partial");
    assert!(ws_cov.unmapped_fields.iter().any(|f| f == "f99"));

    let embedded_cov = coverage
        .category(ExtCategory::EmbeddedProjectConfig)
        .unwrap();
    assert_eq!(embedded_cov.state, "extracted");

    let unmapped_cov = coverage.category(ExtCategory::UnmappedSlots).unwrap();
    assert_eq!(unmapped_cov.unmapped_slots, 2);
    assert_eq!(unmapped_cov.state, "unmapped");

    let page_one = store.ext_websocket_page(source_id, 0, 2).unwrap();
    assert_eq!(page_one.total, 3);
    assert_eq!(page_one.rows.len(), 2);
    assert_eq!(page_one.next_offset, Some(2));
    assert_eq!(page_one.rows[0].item.id, ws_primary.id);
    assert_eq!(page_one.rows[0].connection_id, Some(42));
    assert_eq!(
        page_one.rows[0].direction,
        Some(WebSocketDirection::ClientToServer)
    );
    assert_eq!(page_one.rows[0].kind, Some(WebSocketKind::Binary));
    assert_eq!(
        page_one.rows[0].url.as_deref(),
        Some("wss://ws.example.test/stream")
    );
    assert_eq!(page_one.rows[0].notes.as_deref(), Some("ws-audit-note"));
    assert_eq!(page_one.rows[0].highlight, Some(3));

    let page_two = store.ext_websocket_page(source_id, 2, 2).unwrap();
    assert_eq!(page_two.rows.len(), 1);
    assert_eq!(page_two.next_offset, None);
    assert!(
        page_one.rows[1].item.id == ws_unmapped.id
            && page_two.rows[0].item.id == ws_secondary_only.id
            || page_one.rows[1].item.id == ws_secondary_only.id
                && page_two.rows[0].item.id == ws_unmapped.id
    );

    let directory = tempfile::tempdir().unwrap();
    let saved_path = directory.path().join("ws-archive.mio");
    store.save(Some(&saved_path)).unwrap();
    store.close().unwrap();

    let empty_path = directory.path().join("empty.mio");
    let mut empty_store = ProjectStore::create(None).unwrap();
    empty_store.save(Some(&empty_path)).unwrap();
    empty_store.close().unwrap();
    assert!(
        inspect_mio_ext_archive(&empty_path, 0, 50)
            .unwrap()
            .is_none()
    );

    let (inspected_cov, inspected_ws) = inspect_mio_ext_archive(&saved_path, 0, 50)
        .unwrap()
        .expect("saved project contains External archive");
    assert_eq!(inspected_cov, coverage);
    assert_eq!(inspected_ws.total, 3);

    let raw_frag = read_mio_ext_websocket_fragment(
        &saved_path,
        source_id,
        ws_primary.id,
        false,
        7,
        PAYLOAD_BYTES.len() - 7,
    )
    .unwrap();
    assert!(!raw_frag.edited);
    assert_eq!(raw_frag.offset, 7);
    assert_eq!(raw_frag.bytes, &PAYLOAD_BYTES[7..]);

    let edited_frag = read_mio_ext_websocket_fragment(
        &saved_path,
        source_id,
        ws_primary.id,
        true,
        0,
        EDITED_BYTES.len(),
    )
    .unwrap();
    assert!(edited_frag.edited);
    assert_eq!(edited_frag.bytes, EDITED_BYTES);
}

#[test]
fn websocket_pages_and_fragments_reject_incomplete_out_of_bounds_and_tampered_bindings() {
    let (mut store, source_id) = begin_229(vec![]);
    let (ws_item, messages) = mapped_ws_entry(source_id, "root.f1.f2", 0, 128, false);
    let http_item = ExtItem {
        id: Uuid::new_v4(),
        source_id,
        collection: "root.f1.f0".into(),
        index: 0,
        object_address: Some(64),
        state: "mapped".into(),
        metadata_json: b"{}".to_vec(),
    };
    store
        .put_ext_batch(&[ws_item.clone(), http_item.clone()], &messages)
        .unwrap();

    assert!(store.ext_coverage_report(source_id).is_err());
    assert!(store.ext_websocket_page(source_id, 0, 10).is_err());
    assert!(
        store
            .ext_websocket_fragment(source_id, ws_item.id, false, 0, 4)
            .is_err()
    );

    finish_source(
        &mut store,
        source_id,
        &[ws_item.clone(), http_item.clone()],
        &messages,
    );

    for (offset, limit) in [(0, 0), (0, 501), (usize::MAX, 1), (i64::MAX as usize, 1)] {
        assert!(store.ext_websocket_page(source_id, offset, limit).is_err());
    }
    assert!(store.ext_coverage_report(Uuid::new_v4()).is_err());
    assert!(store.ext_websocket_page(Uuid::new_v4(), 0, 10).is_err());
    assert!(
        store
            .ext_websocket_fragment(Uuid::new_v4(), ws_item.id, false, 0, 1)
            .is_err()
    );
    assert!(
        store
            .ext_websocket_fragment(source_id, http_item.id, false, 0, 1)
            .is_err()
    );
    assert!(
        store
            .ext_websocket_fragment(source_id, ws_item.id, true, 0, 1)
            .is_err()
    );
    for (offset, length) in [
        (0, EXT_FRAGMENT_BYTES + 1),
        (PAYLOAD_BYTES.len() as u64, 1),
        (1, PAYLOAD_BYTES.len()),
        (u64::MAX, 1),
    ] {
        assert!(
            store
                .ext_websocket_fragment(source_id, ws_item.id, false, offset, length)
                .is_err()
        );
    }

    let mut tampered: Value = serde_json::from_slice(&ws_item.metadata_json).unwrap();
    tampered["record"]["mapping"]["value"]["payload"]["binding"]["message_id"] =
        json!(Uuid::new_v4());
    store
        .connection
        .execute(
            "UPDATE ext_items SET metadata=?1 WHERE id=?2",
            rusqlite::params![
                serde_json::to_vec(&tampered).unwrap(),
                ws_item.id.to_string()
            ],
        )
        .unwrap();
    assert!(store.ext_websocket_page(source_id, 0, 10).is_err());
    assert!(
        store
            .ext_websocket_fragment(source_id, ws_item.id, false, 0, 4)
            .is_err()
    );
}
