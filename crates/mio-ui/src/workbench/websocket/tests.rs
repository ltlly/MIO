use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{ScrollDelta, TestAppContext, point};

use super::super::tests::{Fixture, drain, fixture, pump_native, tab};
use super::*;

mod part_1;
mod part_2;
mod part_3;
mod part_4;
mod part_5;
mod part_6;

pub(super) fn message(
    flow_id: FlowId,
    sequence: u64,
    payload: &[u8],
    kind: WebSocketKind,
) -> (WebSocketSummary, WebSocketMessage) {
    let record = WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id,
        sequence,
        direction: WebSocketDirection::ClientToServer,
        kind,
        payload: payload.to_vec(),
        observed_unix_ms: sequence as i64,
    };
    (
        WebSocketSummary {
            id: record.id,
            ordinal: sequence,
            connection_id: 1,
            listener_port: Some(9001),
            revision: record.revision,
            notes_revision: 0,
            notes_preview: String::new(),
            flow_id,
            sequence,
            direction: record.direction,
            kind,
            payload_bytes: payload.len() as u64,
            observed_unix_ms: record.observed_unix_ms,
            url: "ws://127.0.0.1:9001/socket".into(),
            edited: false,
        },
        record,
    )
}

pub(super) fn query(fixture: &mut Fixture) -> (u64, Option<FlowId>, usize, usize) {
    drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::QueryWebSocketHistory {
                project,
                query_revision,
                flow_id,
                offset,
                limit,
                ..
            } => {
                assert_eq!(project, fixture.project);
                Some((query_revision, flow_id, offset, limit))
            }
            _ => None,
        })
        .expect("actual click/event must query WebSocket summaries")
}

pub(super) fn read(fixture: &mut Fixture) -> (u64, Uuid) {
    drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::ReadWebSocket {
                project,
                selection_revision,
                message_id,
            } => {
                assert_eq!(project, fixture.project);
                Some((selection_revision, message_id))
            }
            _ => None,
        })
        .expect("actual row click must request only that message")
}

pub(super) fn open(fixture: &mut Fixture, cx: &mut TestAppContext) -> u64 {
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("proxy-websocket-history", cx)
    })
    .unwrap();
    let (revision, flow, offset, limit) = query(fixture);
    assert_eq!((flow, offset, limit), (None, 0, WS_HISTORY_BATCH_SIZE));
    revision
}

pub(super) fn click_row(fixture: &Fixture, row: usize, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(("websocket-row", row), cx);
    })
    .unwrap();
}

pub(super) fn scroll(fixture: &Fixture, delta: f32, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "websocket-history-table",
            ScrollDelta::Pixels(point(px(0.), px(delta))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
}

pub(super) fn changed(fixture: &Fixture, cx: &mut TestAppContext) {
    fixture
        .events
        .try_send(Event::WebSocketHistoryChanged {
            project: fixture.project,
        })
        .unwrap();
    cx.run_until_parked();
}
