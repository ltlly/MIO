use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{ClipboardItem, Focusable, MouseButton, point};

use super::*;
use crate::workbench::tests::{pump_native, record};

mod part_1;
mod part_2;
mod part_3;

pub(super) fn select_history(
    fixture: &mut Fixture,
    flow: &FlowRecord,
    cx: &mut TestAppContext,
) -> u64 {
    let query_revision = cx.read(|cx| fixture.view.read(cx).query_revision);
    fixture
        .events
        .try_send(Event::HistoryPage {
            project: fixture.project,
            query_revision,
            offset: 0,
            total: 1,
            rows: vec![flow.summary.clone().into()],
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-proxy", cx);
        window.click("proxy-history", cx);
        window.click_at("table", point(px(90.), px(theme::ROW_HEIGHT * 1.5)), cx);
    })
    .unwrap();
    drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::ReadFlow {
                selection_revision,
                flow_id,
                ..
            } if flow_id == flow.summary.id => Some(selection_revision),
            _ => None,
        })
        .expect("clicking history requests its selected body")
}

pub(super) fn queue_history_transfer(fixture: &mut Fixture, cx: &mut TestAppContext) -> u64 {
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("i"), cx)
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadFlow {
            selection_revision, ..
        },
    ] = commands.as_slice()
    else {
        panic!("Send to Intruder reads one request and never sends network traffic");
    };
    *selection_revision
}
