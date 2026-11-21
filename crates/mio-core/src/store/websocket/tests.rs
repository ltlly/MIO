mod integrity;
mod migrations;
mod search;
mod search_corruption;

use std::sync::Arc;

use rusqlite::{
    Connection, OpenFlags,
    hooks::{AuthAction, AuthContext, Authorization},
};
use tokio_util::sync::CancellationToken;

use super::super::{SCHEMA, tests::record as flow_record};
use super::query::{byte_character_view, history_sql, search_page};
use super::records::MAX_EXCLUDED_IDS;
use super::*;
use crate::{FlowState, http::MAX_BODY};

pub(super) fn query(store: &ProjectStore, filter: WebSocketHistoryFilter) -> WebSocketHistoryQuery {
    WebSocketHistoryQuery {
        project: crate::ProjectKey {
            id: store.id,
            generation: 1,
        },
        query_revision: 1,
        filter,
        flow_id: None,
        offset: 0,
        limit: 1,
        selected_message_id: None,
    }
}

pub(super) fn message(flow_id: FlowId, sequence: u64) -> WebSocketMessage {
    WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id,
        sequence,
        direction: WebSocketDirection::ClientToServer,
        kind: WebSocketKind::Binary,
        payload: vec![0, 255],
        observed_unix_ms: 50,
    }
}

pub(super) fn deny_body_reads(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Read {
            table_name: "websocket_payloads" | "messages",
            ..
        }
        | AuthAction::Read {
            table_name: "websocket_originals",
            column_name: "body",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

#[test]
fn metadata_filters_count_and_page_before_slicing_without_reading_payloads() {
    let mut store = ProjectStore::create(None).unwrap();
    let flows = [flow_record(), flow_record(), flow_record()];
    for flow in &flows {
        store.put(flow).unwrap();
    }
    for (flow, sequence, direction, port) in [
        (0, 1, WebSocketDirection::ClientToServer, Some(9091)),
        (0, 2, WebSocketDirection::ServerToClient, Some(9091)),
        (1, 1, WebSocketDirection::ServerToClient, Some(9092)),
        (1, 2, WebSocketDirection::ClientToServer, Some(9092)),
        (2, 1, WebSocketDirection::ServerToClient, None),
    ] {
        let mut record = message(flows[flow].summary.id, sequence);
        record.direction = direction;
        store
            .put_captured_websocket(&CapturedWebSocket {
                record: record.into(),
                listener_port: port,
                original_payload: None,
            })
            .unwrap();
    }
    store.connection.authorizer(Some(deny_body_reads));
    for (filter, expected) in [
        (WebSocketHistoryFilter::default(), vec![1, 2, 3, 4, 5]),
        (
            WebSocketHistoryFilter {
                hide_incoming: true,
                ..Default::default()
            },
            vec![1, 4],
        ),
        (
            WebSocketHistoryFilter {
                hide_outgoing: true,
                ..Default::default()
            },
            vec![2, 3, 5],
        ),
        (
            WebSocketHistoryFilter {
                hide_incoming: true,
                hide_outgoing: true,
                listener_port: None,
                ..Default::default()
            },
            vec![],
        ),
        (
            WebSocketHistoryFilter {
                listener_port: Some(9091),
                ..Default::default()
            },
            vec![1, 2],
        ),
        (
            WebSocketHistoryFilter {
                listener_port: Some(9092),
                hide_outgoing: true,
                ..Default::default()
            },
            vec![3],
        ),
        (
            WebSocketHistoryFilter {
                listener_port: Some(9093),
                ..Default::default()
            },
            vec![],
        ),
    ] {
        let (total, rows) = store
            .websocket_page_with_recovery(None, 0, 10, &[], &[], &filter)
            .unwrap();
        assert_eq!(total, expected.len());
        assert_eq!(
            rows.iter().map(|row| row.ordinal).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            store
                .websocket_page_with_recovery(None, 1, 1, &[], &[], &filter)
                .unwrap(),
            (total, rows.into_iter().skip(1).take(1).collect())
        );
    }
    let filter = WebSocketHistoryFilter {
        hide_outgoing: true,
        listener_port: Some(9092),
        ..Default::default()
    };
    let (total, rows) = store
        .websocket_page_with_recovery(Some(flows[1].summary.id), 0, 10, &[], &[], &filter)
        .unwrap();
    assert_eq!((total, rows[0].ordinal, rows[0].connection_id), (1, 3, 2));
    assert_eq!(
        store
            .websocket_page_with_recovery(Some(Uuid::new_v4()), 0, 10, &[], &[], &filter)
            .unwrap(),
        (0, vec![])
    );
    assert!(
        store
            .websocket_page_with_recovery(
                None,
                0,
                10,
                &[],
                &[],
                &WebSocketHistoryFilter {
                    listener_port: Some(0),
                    ..Default::default()
                }
            )
            .is_err()
    );
}

#[test]
fn pages_are_lazy_ordered_by_insertion_and_exclude_recovery_without_double_counting() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut first_flow = flow_record();
    let second_flow = flow_record();
    store.put(&first_flow).unwrap();
    store.put(&second_flow).unwrap();
    let mut first = message(first_flow.summary.id, 1);
    first.observed_unix_ms = i64::MAX;
    let second = message(second_flow.summary.id, 1);
    let mut third = message(first_flow.summary.id, 2);
    third.observed_unix_ms = i64::MIN;
    for (record, port) in [(&first, 9091), (&second, 9092), (&third, 9091)] {
        store
            .put_captured_websocket(&CapturedWebSocket {
                record: record.clone().into(),
                listener_port: Some(port),
                original_payload: None,
            })
            .unwrap();
    }
    first_flow.summary.revision += 1;
    first_flow.summary.url = "wss://localhost/updated".into();
    first_flow.request.url.clone_from(&first_flow.summary.url);
    store.put(&first_flow).unwrap();
    store.connection.authorizer(Some(deny_body_reads));
    let (total, rows) = store.websocket_page(None, 0, 2, &[]).unwrap();
    assert_eq!(total, 3);
    assert_eq!(
        rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        [first.id, second.id]
    );
    assert_eq!(rows[0].url, first_flow.summary.url);
    assert_eq!(rows[0].observed_unix_ms, i64::MAX);
    assert_eq!(rows[0].payload_bytes, 2);
    assert_eq!(
        (
            rows[0].ordinal,
            rows[0].connection_id,
            rows[0].listener_port
        ),
        (1, 1, Some(9091))
    );
    assert_eq!(
        (
            rows[1].ordinal,
            rows[1].connection_id,
            rows[1].listener_port
        ),
        (2, 2, Some(9092))
    );
    let (total, rows) = store.websocket_page(None, 2, 1, &[]).unwrap();
    assert_eq!((total, rows[0].id), (3, third.id));
    assert_eq!(
        (
            rows[0].ordinal,
            rows[0].connection_id,
            rows[0].listener_port
        ),
        (3, 1, Some(9091))
    );
    let (total, rows) = store
        .websocket_page(Some(first_flow.summary.id), 0, 500, &[third.id, third.id])
        .unwrap();
    assert_eq!((total, rows.len(), rows[0].id), (1, 1, first.id));
    assert_eq!(
        store.flow_url(first_flow.summary.id).unwrap(),
        first_flow.summary.url
    );
    assert!(store.websocket_page(None, 3, 1, &[]).unwrap().1.is_empty());
    assert!(
        store.read_websocket(first.id).is_err(),
        "the authorizer actually blocks payload reads"
    );
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(store.read_websocket(first.id).unwrap(), first);
}

#[test]
fn binary_text_controls_and_close_bytes_survive_save_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("websocket.mio");
    let mut store = ProjectStore::create(None).unwrap();
    let flow = flow_record();
    store.put(&flow).unwrap();
    let mut close = 1000u16.to_be_bytes().to_vec();
    close.extend_from_slice("中文\0reason".as_bytes());
    let records: Vec<_> = [
        (WebSocketKind::Binary, (0..=255).collect()),
        (WebSocketKind::Text, "UTF-8\0中文".as_bytes().to_vec()),
        (WebSocketKind::Ping, vec![0, 255]),
        (WebSocketKind::Pong, vec![255, 0]),
        (WebSocketKind::Close, close),
        (WebSocketKind::Close, vec![]),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (kind, payload))| {
        let mut record = message(flow.summary.id, index as u64 + 1);
        record.kind = kind;
        record.payload = payload;
        if index % 2 == 1 {
            record.direction = WebSocketDirection::ServerToClient;
        }
        record
    })
    .collect();
    for record in &records {
        store.put_websocket(record).unwrap();
    }
    let before = store.websocket_page(None, 0, 500, &[]).unwrap();
    for record in records.iter().rev() {
        store.put_websocket(record).unwrap();
    }
    assert_eq!(store.websocket_page(None, 0, 500, &[]).unwrap(), before);
    let project_id = store.id;
    store.save(Some(&path)).unwrap();
    drop(store);
    let mut reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(reopened.id, project_id);
    assert_eq!(reopened.websocket_page(None, 0, 500, &[]).unwrap(), before);
    for record in &records {
        assert_eq!(reopened.read_websocket(record.id).unwrap(), *record);
    }
    let mut pending_flow = flow.clone();
    pending_flow.summary.revision += 1;
    pending_flow.summary.state = FlowState::Pending;
    reopened.put(&pending_flow).unwrap();
    let only_websocket = message(flow.summary.id, 7);
    reopened
        .save_with(
            None,
            None,
            &[],
            &[only_websocket.clone().into()],
            &[],
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(
        reopened.read_websocket(only_websocket.id).unwrap(),
        only_websocket
    );
    pending_flow.summary.revision += 1;
    pending_flow.summary.state = FlowState::Cancelled;
    assert_eq!(
        reopened.read(flow.summary.id).unwrap(),
        pending_flow,
        "WS-only recovery follows producer Stop and must not leave the handshake in flight"
    );
    drop(reopened);
    let reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(reopened.read(flow.summary.id).unwrap(), pending_flow);
    assert_eq!(
        reopened.read_websocket(only_websocket.id).unwrap(),
        only_websocket
    );
}
