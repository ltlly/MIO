use gpui_kit::test::TestWindowExt;
use gpui_kit::{EntityInputHandler, TestAppContext, point};
use mio_core::{ExtMessagePage, ExtMessageSummary};

use super::super::tests::report;
use super::*;

mod ack;
use crate::workbench::tests::{Fixture, drain, fixture, project_menu, pump_native, tab};

const RAW: &[u8] = b"POST /archived HTTP/1.1\r\nX-A: one\r\nX-A: two\r\n\r\n\0\xff";

fn click(fixture: &Fixture, id: &'static str, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
}

fn browse(fixture: &mut Fixture, cx: &mut TestAppContext) -> [ExtMessageSummary; 2] {
    drain(&mut fixture.commands);
    project_menu(fixture, "查看 外部导入记录", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::QueryExtReport { query_revision, .. }] = commands.as_slice() else {
        panic!("query report")
    };
    let source_id = Uuid::new_v4();
    fixture
        .events
        .try_send(Event::ExtReport {
            project: fixture.project,
            query_revision: *query_revision,
            result: Ok(Some(ExtSourceInfo {
                id: source_id,
                report: report(),
            })),
        })
        .unwrap();
    cx.run_until_parked();
    click(fixture, "ext-browse-messages", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::QueryExtMessages { query_revision, .. }] = commands.as_slice() else {
        panic!("query messages")
    };
    let rows = std::array::from_fn(|index| ExtMessageSummary {
        id: Uuid::new_v4(),
        source_id,
        item_id: Uuid::new_v4(),
        collection: "root.f2.f1".into(),
        item_index: index as u64,
        role: "current_request".into(),
        field_id: 4,
        raw_offset: 0,
        raw_bytes: RAW.len() as u64,
        body_offset: Some(RAW.len() as u64 - 2),
        declared_version: Some(HttpVersion::Http11),
        method: Some("POST".into()),
        request_target: Some("/archived".into()),
        status: None,
        diagnostic: None,
    });
    fixture
        .events
        .try_send(Event::ExtMessages {
            project: fixture.project,
            query_revision: *query_revision,
            source_id,
            collection: None,
            offset: 0,
            result: Ok(ExtMessagePage {
                total: 2,
                rows: rows.to_vec(),
                next_offset: None,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    select(fixture, &rows[0], 0, cx);
    rows
}

fn select(
    fixture: &mut Fixture,
    summary: &ExtMessageSummary,
    row: usize,
    cx: &mut TestAppContext,
) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click_at(
            "table",
            point(px(90.), px(theme::ROW_HEIGHT * (row as f32 + 1.5))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadExtMessage {
            selection_revision,
            message_id,
            offset: 0,
            limit,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("read only the selected fragment")
    };
    assert_eq!((*message_id, *limit), (summary.id, RAW.len()));
    fixture
        .events
        .try_send(Event::ExtMessage {
            project: fixture.project,
            selection_revision: *selection_revision,
            message_id: *message_id,
            offset: 0,
            result: Ok(ExtMessageFragment {
                summary: summary.clone(),
                offset: 0,
                bytes: RAW.to_vec(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
}

fn request(fixture: &mut Fixture, cx: &mut TestAppContext) -> Command {
    click(fixture, "ext-open-repeater", cx);
    let mut commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 1);
    let command = commands.pop().unwrap();
    assert!(
        matches!(command, Command::OpenExtRepeater { .. }),
        "the button must request the selected archive only"
    );
    command
}

fn proposal(command: &Command, tab: WorkTab) -> Event {
    let Command::OpenExtRepeater {
        project,
        operation_id,
        source_id,
        item_id,
        message_id,
        workspace_revision,
    } = command
    else {
        panic!("archive command")
    };
    Event::ExtRepeaterOpened {
        project: *project,
        operation_id: *operation_id,
        source_id: *source_id,
        item_id: *item_id,
        message_id: *message_id,
        workspace_revision: *workspace_revision,
        result: Ok(tab),
    }
}

fn archived() -> WorkTab {
    let mut result = tab("外部归档请求");
    result.draft.target.clear();
    result.draft.raw = RAW.to_vec();
    result
}

fn saved(fixture: &mut Fixture, cx: &mut TestAppContext) -> Workspace {
    let commands = drain(&mut fixture.commands);
    let commands = crate::workbench::tests::acknowledge_history_query(fixture, commands, cx);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("only a workspace save is allowed, never an automatic Send")
    };
    workspace.clone()
}

mod part_1;
