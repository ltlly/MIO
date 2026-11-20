use super::*;
use crate::{FlowRecord, FlowState, HttpHistoryFilter, HttpHistorySort, ProjectKey};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use serde_json::json;

mod bindings;
mod duplicates;
mod mime_listener;
mod originals;

mod part_metadata;
mod part_params_basic;
mod part_params_limits;
mod part_search;

fn query(store: &ProjectStore) -> HttpHistoryQuery {
    HttpHistoryQuery {
        project: ProjectKey {
            id: store.id,
            generation: 1,
        },
        query_revision: 1,
        offset: 0,
        limit: 200,
        url_filter: String::new(),
        filter: HttpHistoryFilter::default(),
        sort: HttpHistorySort {
            column: HttpHistorySortColumn::Status,
            ascending: true,
        },
        selected_ids: Vec::new(),
    }
}

fn live(url: &str, status: Option<u16>, time: i64, source: TrafficSource) -> FlowRecord {
    let mut row = crate::store::tests::record();
    row.summary.id = Uuid::new_v4();
    row.summary.source = source;
    row.summary.started_unix_ms = time;
    row.summary.url = url.into();
    row.request.url = url.into();
    row.summary.method = "GET".into();
    row.request.method = "GET".into();
    row.summary.status = status;
    row.response = status.map(|status| crate::HttpResponse {
        status,
        version: crate::HttpVersion::Http11,
        reason_phrase: None,
        headers: Vec::new(),
        trailers: Vec::new(),
        body: b"untouched response body".to_vec(),
    });
    row.summary.response_bytes = row.response.as_ref().map_or(0, |r| r.body.len() as u64);
    row.summary.state = if status.is_some() {
        FlowState::Complete
    } else {
        FlowState::Pending
    };
    row
}

fn no_bodies(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Read {
            table_name: "messages",
            column_name: "body",
        }
        | AuthAction::Read {
            table_name: "ext_source_chunks",
            column_name: "data",
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}
