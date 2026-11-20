use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, point};
use mio_core::ExtItemPage;

use super::super::super::super::tests::{Fixture, drain, fixture, project_menu, pump_native, tab};
use super::super::super::tests::report;
use super::super::tests::{click, fragment, read, summary};
use super::*;

fn open(fixture: &mut Fixture, cx: &mut TestAppContext) -> (Uuid, u64) {
    drain(&mut fixture.commands);
    project_menu(fixture, "查看 外部导入记录", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::QueryExtReport { query_revision, .. }] = commands.as_slice() else {
        panic!("the native report menu action queries provenance")
    };
    let source = Uuid::new_v4();
    fixture
        .events
        .try_send(Event::ExtReport {
            project: fixture.project,
            query_revision: *query_revision,
            result: Ok(Some(ExtSourceInfo {
                id: source,
                report: report(),
            })),
        })
        .unwrap();
    cx.run_until_parked();
    click(fixture, "ext-browse-items", cx);
    let (revision, filter, offset) = query(fixture, source);
    assert_eq!((filter, offset), (None, 0));
    (source, revision)
}

fn query(fixture: &mut Fixture, source: Uuid) -> (u64, Option<String>, usize) {
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryExtItems {
            project,
            source_id,
            query_revision,
            collection,
            offset,
            limit,
        },
    ] = commands.as_slice()
    else {
        panic!("the real item query must not load metadata or global messages")
    };
    assert_eq!(
        (*project, *source_id, *limit),
        (fixture.project, source, PAGE_SIZE)
    );
    (*query_revision, collection.clone(), *offset)
}

fn selected(fixture: &mut Fixture, source: Uuid) -> (u64, Uuid, usize) {
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadExtItem {
            project,
            source_id,
            selection_revision,
            item_id,
            message_offset,
            message_limit,
        },
    ] = commands.as_slice()
    else {
        panic!("item selection and message pagination must use ReadExtItem only")
    };
    assert_eq!(
        (*project, *source_id, *message_limit),
        (fixture.project, source, PAGE_SIZE)
    );
    (*selection_revision, *item_id, *message_offset)
}

fn row(source_id: Uuid, index: u64, state: &str, message_count: usize) -> ExtItemSummary {
    ExtItemSummary {
        id: Uuid::new_v4(),
        source_id,
        collection: "root.f2.f1".into(),
        index,
        object_address: (state != "null").then_some(72),
        state: state.into(),
        message_count,
    }
}

fn message(item: &ExtItemSummary, bytes: u64) -> ExtMessageSummary {
    let mut message = summary(item.source_id, item.index, bytes);
    message.item_id = item.id;
    message.collection = item.collection.clone();
    message.role = "current_request".into();
    message
}

fn page(
    fixture: &Fixture,
    source: Uuid,
    revision: u64,
    filter: Option<String>,
    offset: usize,
    total: usize,
    rows: Vec<ExtItemSummary>,
) {
    fixture
        .events
        .try_send(Event::ExtItems {
            project: fixture.project,
            query_revision: revision,
            source_id: source,
            collection: filter,
            offset,
            result: Ok(ExtItemPage {
                total,
                next_offset: (offset + rows.len() < total).then_some(offset + rows.len()),
                rows,
            }),
        })
        .unwrap();
}

fn detail(
    item: &ExtItemSummary,
    metadata: &[u8],
    offset: usize,
    rows: Vec<ExtMessageSummary>,
) -> ExtItemDetail {
    ExtItemDetail {
        item: ExtItem {
            id: item.id,
            source_id: item.source_id,
            collection: item.collection.clone(),
            index: item.index,
            object_address: item.object_address,
            state: item.state.clone(),
            metadata_json: metadata.to_vec(),
        },
        messages: ExtMessagePage {
            total: item.message_count,
            next_offset: (offset + rows.len() < item.message_count).then_some(offset + rows.len()),
            rows,
        },
    }
}

fn deliver(
    fixture: &Fixture,
    source: Uuid,
    request: (u64, Uuid, usize),
    result: Result<ExtItemDetail, String>,
) {
    fixture
        .events
        .try_send(Event::ExtItem {
            project: fixture.project,
            selection_revision: request.0,
            source_id: source,
            item_id: request.1,
            message_offset: request.2,
            result,
        })
        .unwrap();
}

fn select(fixture: &Fixture, table: &'static str, row: usize, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.within(table).click_at(
            "table",
            point(px(90.), px(theme::ROW_HEIGHT * (row as f32 + 1.5))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
}

fn choose(fixture: &Fixture, query: &str, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window
            .within("ext-item-collection-select")
            .click("input", cx);
        window.input(query, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
}

mod part_1;
mod part_2;
mod part_3;
