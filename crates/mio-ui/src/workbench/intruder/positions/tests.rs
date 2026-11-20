use gpui_kit::test::TestWindowExt;
use gpui_kit::{ClipboardItem, EntityInputHandler, TestAppContext};

use super::super::tests::{attack, intruder_fixture};
use super::*;
use crate::workbench::tests::{Fixture, drain};

mod part_1;
mod part_2;

pub(super) fn mark(id: &str, start: usize, end: usize) -> Mark {
    Mark {
        id: id.into(),
        position: Position { start, end },
    }
}

pub(super) fn snapshot(
    fixture: &Fixture,
    id: TabId,
    cx: &mut TestAppContext,
) -> (Vec<u8>, Vec<Position>) {
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.intruder.editors[&id]
                .request
                .snapshot(window, cx)
                .unwrap()
        })
    })
    .unwrap()
}

pub(super) fn automatic_fixture(cx: &mut TestAppContext) -> (Fixture, TabId, Vec<u8>) {
    let mut original = attack();
    let id = original.id;
    let raw = "GET /?q=one&literal=§ HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n"
        .as_bytes()
        .to_vec();
    original.spec.draft.raw = raw.clone();
    original.spec.positions.clear();
    (intruder_fixture(cx, vec![original], 64), id, raw)
}

pub(super) fn auto_request(
    fixture: &mut Fixture,
    cx: &mut TestAppContext,
) -> (Uuid, Vec<Position>) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click("intruder-position-auto", cx);
        window.click("intruder-position-auto", cx);
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    let [
        Command::AutoIntruderPositions {
            operation_id,
            raw,
            selection,
            placement,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("Auto emits exactly one bounded parsing command and no network request");
    };
    (
        *operation_id,
        mio_core::intruder::automatic_positions(raw, selection.clone(), *placement).unwrap(),
    )
}
