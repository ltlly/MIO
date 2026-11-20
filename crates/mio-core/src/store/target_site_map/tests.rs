mod archive;
mod tree;

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use serde_json::json;

use super::*;
use crate::{
    ExtCollectionReport, ExtImportReport, ExtItem, ExtMessageSummary, FlowState, HttpResponse,
    HttpVersion, ScopeRule, TrafficSource,
};

fn live_flow(
    url: &str,
    method: &str,
    status: Option<u16>,
    source: TrafficSource,
    state: FlowState,
) -> crate::FlowRecord {
    let mut record = crate::store::tests::record();
    record.summary.id = crate::Uuid::new_v4();
    record.summary.source = source;
    record.summary.url = url.into();
    record.request.url = url.into();
    record.summary.method = method.into();
    record.request.method = method.into();
    record.summary.status = status;
    record.response = status.map(|status| HttpResponse {
        status,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: Vec::new(),
        trailers: Vec::new(),
        body: b"bounded body".to_vec(),
    });
    record.summary.response_bytes = record
        .response
        .as_ref()
        .map_or(0, |response| response.body.len() as u64);
    record.summary.state = state;
    record
}

struct ArchiveSpec {
    source_id: crate::Uuid,
    bytes: Vec<u8>,
    items: Vec<ExtItem>,
    messages: Vec<ExtMessageSummary>,
}

struct ArchiveItemSpec<'a> {
    collection: &'a str,
    index: u64,
    address: u64,
    host: &'a str,
    port: u16,
    tls: bool,
    method: &'a str,
    target: &'a str,
    status: u16,
}

impl ArchiveSpec {
    fn new(source_id: crate::Uuid) -> Self {
        Self {
            source_id,
            bytes: Vec::new(),
            items: Vec::new(),
            messages: Vec::new(),
        }
    }

    fn add_item(&mut self, spec: ArchiveItemSpec<'_>) -> crate::Uuid {
        let request = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\n\r\n",
            spec.method, spec.target, spec.host
        );
        let response = format!(
            "HTTP/1.1 {} Test\r\nContent-Length: 2\r\n\r\nok",
            spec.status
        );
        let item_id = crate::Uuid::new_v4();
        let request_offset = self.bytes.len() as u64 + 8;
        self.bytes.extend_from_slice(&[0; 8]);
        self.bytes.extend_from_slice(request.as_bytes());
        let response_offset = self.bytes.len() as u64 + 8;
        self.bytes.extend_from_slice(&[0; 8]);
        self.bytes.extend_from_slice(response.as_bytes());
        let request_message = ExtMessageSummary {
            id: crate::Uuid::new_v4(),
            source_id: self.source_id,
            item_id,
            collection: spec.collection.into(),
            item_index: spec.index,
            role: "request".into(),
            field_id: 15,
            raw_offset: request_offset,
            raw_bytes: request.len() as u64,
            body_offset: Some(request.len() as u64),
            declared_version: Some(HttpVersion::Http11),
            method: Some(spec.method.into()),
            request_target: Some(spec.target.into()),
            status: None,
            diagnostic: None,
        };
        let response_message = ExtMessageSummary {
            id: crate::Uuid::new_v4(),
            source_id: self.source_id,
            item_id,
            collection: spec.collection.into(),
            item_index: spec.index,
            role: "response".into(),
            field_id: 18,
            raw_offset: response_offset,
            raw_bytes: response.len() as u64,
            body_offset: Some(response.len() as u64 - 2),
            declared_version: Some(HttpVersion::Http11),
            method: None,
            request_target: None,
            status: Some(spec.status),
            diagnostic: None,
        };
        let bytes_field = |offset: u64, length: u64| {
            json!({
                "kind": "bytes", "address": offset - 8, "payload_bytes": length,
                "element_count": length, "total_size": length + 8,
            })
        };
        let service = json!({
            "id": 0,
            "observation": {"state": "value", "value": {
                "object": {"address": 600, "object_type": 0, "subtype": 0},
                "mapping": {"state": "value", "value": {
                    "host": {"id": 0, "observation": {"state": "value", "value": {
                        "object": {"address": 700, "object_type": 0, "subtype": 0},
                        "mapping": {"state": "value", "value": spec.host},
                    }}},
                    "port": {"id": 1, "observation": {"state": "value", "value": spec.port}},
                    "tls": {"id": 2, "observation": {"state": "value", "value": spec.tls}},
                }},
            }},
        });
        let parsed_request = json!({
            "id": 2,
            "observation": {"state": "value", "value": {
                "object": {"address": 500, "object_type": 0, "subtype": 0},
                "mapping": {"state": "value", "value": {"service": service}},
            }},
        });
        let request_value = bytes_field(request_offset, request.len() as u64);
        let response_value = bytes_field(response_offset, response.len() as u64);
        let request_field = json!({
            "id": 15,
            "observation": {"state": "value", "value": request_value},
            "binding": {
                "source_id": self.source_id,
                "item_id": item_id,
                "message_id": request_message.id,
            },
            "parsed_request": parsed_request,
        });
        let response_field = json!({
            "id": 18,
            "observation": {"state": "value", "value": response_value},
        });
        let metadata = json!({
            "version": 1,
            "context": "http_history_229",
            "slot": {"state": "value", "value": spec.address},
            "record": {
                "object": {"address": spec.address, "object_type": 1, "subtype": 0},
                "mapping": {"state": "value", "value": {
                    "request": request_field,
                    "response": response_field,
                }},
            },
        });
        self.items.push(ExtItem {
            id: item_id,
            source_id: self.source_id,
            collection: spec.collection.into(),
            index: spec.index,
            object_address: Some(spec.address),
            state: "mapped".into(),
            metadata_json: serde_json::to_vec(&metadata).unwrap(),
        });
        self.messages.push(request_message);
        self.messages.push(response_message);
        item_id
    }

    fn add_null_slot(&mut self, collection: &str, index: u64) {
        let metadata = json!({
            "version": 1,
            "context": "http_history_229",
            "slot": {"state": "null", "value": null},
            "record": {
                "object": {"address": 0, "object_type": 1, "subtype": 0},
                "mapping": {"state": "failed"},
            },
        });
        self.items.push(ExtItem {
            id: crate::Uuid::new_v4(),
            source_id: self.source_id,
            collection: collection.into(),
            index,
            object_address: None,
            state: "null".into(),
            metadata_json: serde_json::to_vec(&metadata).unwrap(),
        });
    }
}

fn load_archive(store: &mut crate::store::ProjectStore, spec: ArchiveSpec) -> crate::Uuid {
    let mut collections = Vec::new();
    for collection in EXT_PROXY_COLLECTIONS {
        let items: Vec<_> = spec
            .items
            .iter()
            .filter(|item| item.collection == collection)
            .collect();
        if items.is_empty() {
            continue;
        }
        let message_count = spec
            .messages
            .iter()
            .filter(|message| message.collection == collection)
            .count() as u64;
        collections.push(ExtCollectionReport {
            path: collection.into(),
            logical_items: items.iter().map(|item| item.index + 1).max(),
            indexed_items: items.len() as u64,
            message_count,
            state: "indexed".into(),
        });
    }
    let mut report = ExtImportReport {
        schema_floor: 229,
        schema_revision: 229,
        source_bytes: spec.bytes.len() as u64,
        indexed_items: spec.items.len() as u64,
        indexed_messages: spec.messages.len() as u64,
        collections,
        repeater_active_tab: None,
        gaps: Vec::new(),
        complete_archive: false,
    };
    store.begin_source(spec.source_id, &report).unwrap();
    store
        .append_source_chunk(spec.source_id, 0, &spec.bytes)
        .unwrap();
    store.put_ext_batch(&spec.items, &spec.messages).unwrap();
    report.complete_archive = true;
    store.finish_source(spec.source_id, &report).unwrap();
    spec.source_id
}

fn scope_rule(prefix: &str) -> ScopeRule {
    ScopeRule {
        id: crate::Uuid::new_v4(),
        enabled: true,
        prefix: prefix.into(),
        include_subdomains: false,
    }
}

fn deny_bodies(context: AuthContext<'_>) -> Authorization {
    if matches!(
        context.action,
        AuthAction::Read {
            table_name: "messages",
            column_name: "body"
        } | AuthAction::Read {
            table_name: "ext_source_chunks",
            column_name: "data"
        }
    ) {
        Authorization::Deny
    } else {
        Authorization::Allow
    }
}

#[test]
fn site_map_snapshot_extracts_referer_and_redirect_location_crawl_edges() {
    let mut store = crate::store::ProjectStore::create(None).unwrap();
    let mut f_login = live_flow(
        "https://app.example.com:443/login",
        "POST",
        Some(302),
        TrafficSource::Proxy,
        FlowState::Complete,
    );
    f_login.summary.started_unix_ms = 100;
    if let Some(resp) = f_login.response.as_mut() {
        resp.headers.push(crate::Header {
            name: "Location".into(),
            value: b"/dashboard".to_vec(),
        });
    }
    store.put(&f_login).unwrap();

    let mut f_dash = live_flow(
        "https://app.example.com:443/dashboard",
        "GET",
        Some(200),
        TrafficSource::Proxy,
        FlowState::Complete,
    );
    f_dash.summary.started_unix_ms = 200;
    store.put(&f_dash).unwrap();

    let mut f_profile = live_flow(
        "https://app.example.com:443/api/profile",
        "GET",
        Some(200),
        TrafficSource::Proxy,
        FlowState::Complete,
    );
    f_profile.summary.started_unix_ms = 300;
    f_profile.request.headers.push(crate::Header {
        name: "Referer".into(),
        value: b"https://app.example.com:443/dashboard".to_vec(),
    });
    store.put(&f_profile).unwrap();

    let snap = store
        .target_site_map_snapshot(& crate::TargetScope::default(), &SiteMapQuery::default(), &[])
        .unwrap();
    assert_eq!(snap.site_map_items.len(), 3);
    assert_eq!(
        snap.referer_for(crate::HistoryEntryId::Live(f_login.summary.id)),
        None
    );
    assert_eq!(
        snap.referer_for(crate::HistoryEntryId::Live(f_dash.summary.id)),
        Some("https://app.example.com:443/login")
    );
    assert_eq!(
        snap.referer_for(crate::HistoryEntryId::Live(f_profile.summary.id)),
        Some("https://app.example.com/dashboard")
    );
}
