use super::*;
use crate::{ExtCollectionReport, ExtImportReport, ExtItem, HttpVersion};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const REQUEST: &[u8] = b"POST /fixture HTTP/1.1\r\nHost: header.invalid\r\nX-A:first\r\nX-A:\tsecond\r\nContent-Length: 3\r\n\r\n\0\xff\x80";
const EDITED_REQUEST: &[u8] = b"PUT /edited HTTP/1.1\r\nHost: edited-header.invalid\r\nX-A:first\r\nX-A:\tsecond\r\nX-Edited: yes\r\nContent-Length: 3\r\n\r\n\xff\0\x81";

fn archived(row: &HistoryRow) -> &ExtHistoryRow {
    let HistoryRow::ExtProxy(archive) = row else {
        panic!("expected archived Proxy row")
    };
    archive
}

fn field(id: u8, value: Value) -> Value {
    json!({"id": id, "observation": {"state": "value", "value": value}})
}

fn decoded(address: u64, value: Value) -> Value {
    json!({"object": {"address": address, "object_type": 0, "subtype": 0}, "mapping": {"state": "value", "value": value}})
}

fn response() -> Vec<u8> {
    let mut bytes =
        b"HTTP/1.1 201 Created\r\nContent-Length: 3\r\nX-A: first\r\nX-A:\tsecond\r\nX-Pad: "
            .to_vec();
    bytes.resize(139 - 7, b'p');
    bytes.extend_from_slice(b"\r\n\r\n\0\xff\x80");
    bytes
}

fn changed_response() -> Vec<u8> {
    let mut bytes = b"HTTP/1.1 202 Accepted\r\nContent-Length: 3\r\nX-A: first\r\nX-A:\tsecond\r\nX-Edited: yes\r\nX-Pad: ".to_vec();
    bytes.resize(155 - 7, b'p');
    bytes.extend_from_slice(b"\r\n\r\n\xff\0\x81");
    bytes
}

pub(in crate::store::ext_archive) fn fixture(
    extra: usize,
    edited: bool,
    response_edited: bool,
) -> (ProjectStore, Uuid, Vec<ExtItem>) {
    let mut response = response();
    if response_edited {
        response.insert(response.len() - 7, b'p');
    }
    let edited_response = changed_response();
    let mut archive = [vec![0; 8], REQUEST.to_vec(), vec![0; 8], response.clone()].concat();
    let response_offset = (16 + REQUEST.len()) as u64;
    let edited_offset = archive.len() as u64 + 8;
    if edited {
        archive.extend_from_slice(&[0; 8]);
        archive.extend_from_slice(EDITED_REQUEST);
    }
    let edited_response_offset = archive.len() as u64 + 8;
    if response_edited {
        archive.extend_from_slice(&[0; 8]);
        archive.extend_from_slice(&edited_response);
    }
    let mut store = ProjectStore::create(None).unwrap();
    let source = Uuid::new_v4();
    let mut report = ExtImportReport {
        schema_floor: 229,
        schema_revision: 229,
        source_bytes: archive.len() as u64,
        indexed_items: 0,
        indexed_messages: 0,
        collections: Vec::new(),
        repeater_active_tab: None,
        gaps: vec!["Synthetic Proxy history fixture".into()],
        complete_archive: false,
    };
    store.begin_source(source, &report).unwrap();
    store.append_source_chunk(source, 0, &archive).unwrap();
    let mut slots = vec![
        ("root.f1.f0", 0, 1000, "mapped"),
        ("root.f1.f1", 0, 1000, "mapped"),
        ("root.f1.f1", 1, 2000, "mapped"),
        ("root.f1.f0", 2, 0, "null"),
        ("root.f1.f1", 2, 1000, "failed"),
        ("root.f1.f2", 0, 3000, "unmapped"),
    ];
    for index in 0..extra {
        slots.push((
            "root.f1.f0",
            10 + index as u64,
            4000 + index as u64,
            "mapped",
        ));
    }
    let mut items = Vec::new();
    let mut messages = Vec::new();
    for (collection, index, address, state) in slots {
        let mut item = ExtItem {
            id: Uuid::new_v4(),
            source_id: source,
            collection: collection.into(),
            index,
            object_address: (address != 0).then_some(72 + items.len() as u64),
            state: state.into(),
            metadata_json: Vec::new(),
        };
        let mut metadata = json!({
            "version": 1,
            "context": if collection == "root.f1.f2" {
                "unmapped_collection"
            } else {
                "http_history_229"
            },
            "slot": {"state": if address == 0 { "null" } else { "value" }, "value": item.object_address},
            "record": {"object": {"address": address, "object_type": 1, "subtype": 0}, "mapping": {"state": if state == "mapped" { "value" } else { "failed" }}},
        });
        if state == "mapped" {
            let mut fields = json!({});
            let mut variants = vec![
                (15, REQUEST, 8, "request"),
                (18, response.as_slice(), response_offset, "response"),
            ];
            if edited {
                variants.push((17, EDITED_REQUEST, edited_offset, "edited_request"));
            }
            if response_edited {
                variants.push((
                    20,
                    edited_response.as_slice(),
                    edited_response_offset,
                    "edited_response",
                ));
            }
            for (field_id, raw, offset, role) in variants {
                let message = ExtMessageSummary {
                    id: Uuid::new_v4(),
                    source_id: source,
                    item_id: item.id,
                    collection: collection.into(),
                    item_index: index,
                    role: role.into(),
                    field_id,
                    raw_offset: offset,
                    raw_bytes: raw.len() as u64,
                    body_offset: raw
                        .windows(4)
                        .position(|bytes| bytes == b"\r\n\r\n")
                        .map(|index| index as u64 + 4),
                    declared_version: Some(HttpVersion::Http11),
                    method: match field_id {
                        15 => Some("POST".into()),
                        17 => Some("PUT".into()),
                        _ => None,
                    },
                    request_target: match field_id {
                        15 => Some("/fixture".into()),
                        17 => Some("/edited".into()),
                        _ => None,
                    },
                    status: match field_id {
                        18 => Some(201),
                        20 => Some(202),
                        _ => None,
                    },
                    diagnostic: None,
                };
                let mut value = field(
                    field_id,
                    json!({
                        "kind": "bytes", "address": offset - 8, "payload_bytes": raw.len(),
                        "element_count": raw.len(), "total_size": raw.len() + 8,
                    }),
                );
                if matches!(field_id, 15 | 17 | 20) {
                    value["binding"] =
                        json!({"source_id": source, "item_id": item.id, "message_id": message.id});
                }
                if matches!(field_id, 15 | 17) {
                    value["parsed_request"] = field(
                        2,
                        decoded(
                            500,
                            json!({
                                "service": field(0, decoded(600, json!({
                                    "host": field(0, decoded(700, json!("127.0.0.1"))),
                                    "port": field(1, json!(if address == 2000 { 18766 } else { 18765 })),
                                    "tls": field(2, json!(false)),
                                }))),
                            }),
                        ),
                    );
                }
                fields[role] = value;
                messages.push(message);
            }
            metadata["record"]["mapping"]["value"] = fields;
        }
        item.metadata_json = serde_json::to_vec(&metadata).unwrap();
        items.push(item);
    }
    store.put_ext_batch(&items, &messages).unwrap();
    for collection in ["root.f1.f0", "root.f1.f1", "root.f1.f2"] {
        let selected: Vec<_> = items
            .iter()
            .filter(|item| item.collection == collection)
            .collect();
        report.collections.push(ExtCollectionReport {
            path: collection.into(),
            logical_items: selected.iter().map(|item| item.index + 1).max(),
            indexed_items: selected.len() as u64,
            message_count: messages
                .iter()
                .filter(|message| message.collection == collection)
                .count() as u64,
            state: "indexed".into(),
        });
    }
    report.indexed_items = items.len() as u64;
    report.indexed_messages = messages.len() as u64;
    report.complete_archive = true;
    store.finish_source(source, &report).unwrap();
    (store, source, items)
}

mod part_aliases;
mod part_delete;
mod part_live_isolation;
mod part_logger;
mod part_targets;
mod part_versions;

fn proxy_page(
    store: &ProjectStore,
    offset: usize,
    limit: usize,
    filter: &str,
) -> Result<(usize, Vec<HistoryRow>)> {
    let result = store.query_http_history(
        &crate::HttpHistoryQuery {
            project: crate::ProjectKey {
                id: store.id,
                generation: 1,
            },
            query_revision: 1,
            offset,
            limit,
            url_filter: filter.into(),
            filter: crate::HttpHistoryFilter::default(),
            sort: crate::HttpHistorySort::default(),
            selected_ids: Vec::new(),
        },
        &CancellationToken::new(),
    )?;
    Ok((result.total, result.rows))
}
