use super::*;

pub(in crate::workbench) fn click(fixture: &Fixture, id: &'static str, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
}

pub(in crate::workbench) fn query(fixture: &mut Fixture) -> (u64, Option<String>, usize) {
    drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::QueryExtMessages {
                project,
                query_revision,
                collection,
                offset,
                limit,
                ..
            } => {
                assert_eq!(project, fixture.project);
                assert_eq!(limit, PAGE_SIZE);
                Some((query_revision, collection, offset))
            }
            _ => None,
        })
        .expect("a real browse/filter/page click must query only summaries")
}

pub(in crate::workbench) fn read(fixture: &mut Fixture) -> (u64, Uuid, u64, usize) {
    drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::ReadExtMessage {
                project,
                selection_revision,
                message_id,
                offset,
                limit,
            } => {
                assert_eq!(project, fixture.project);
                assert!(limit <= FRAGMENT_BYTES);
                Some((selection_revision, message_id, offset, limit))
            }
            _ => None,
        })
        .expect("a real row/fragment click must read exactly that bounded fragment")
}

pub(in crate::workbench) fn open(fixture: &mut Fixture, cx: &mut TestAppContext) -> (Uuid, u64) {
    open_report(fixture, report(), cx)
}

pub(in crate::workbench) fn open_report(
    fixture: &mut Fixture,
    report: ExtImportReport,
    cx: &mut TestAppContext,
) -> (Uuid, u64) {
    drain(&mut fixture.commands);
    project_menu(fixture, "查看 外部导入记录", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::QueryExtReport { query_revision, .. }] = commands.as_slice() else {
        panic!("read project provenance")
    };
    let id = Uuid::new_v4();
    fixture
        .events
        .try_send(Event::ExtReport {
            project: fixture.project,
            query_revision: *query_revision,
            result: Ok(Some(ExtSourceInfo { id, report })),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    click(fixture, "ext-browse-messages", cx);
    let (revision, filter, offset) = query(fixture);
    assert_eq!((filter, offset), (None, 0));
    (id, revision)
}

pub(in crate::workbench) fn summary(
    source_id: Uuid,
    index: u64,
    raw_bytes: u64,
) -> ExtMessageSummary {
    ExtMessageSummary {
        id: Uuid::new_v4(),
        source_id,
        item_id: Uuid::new_v4(),
        collection: "root.f2.f1".into(),
        item_index: index,
        role: "repeater.current_request".into(),
        field_id: 4,
        raw_offset: 128,
        raw_bytes,
        body_offset: None,
        declared_version: Some(HttpVersion::Http11),
        method: Some("GET".into()),
        request_target: Some("/synthetic".into()),
        status: None,
        diagnostic: None,
    }
}

pub(in crate::workbench) fn page(
    fixture: &Fixture,
    source_id: Uuid,
    query_revision: u64,
    collection: Option<String>,
    offset: usize,
    total: usize,
    rows: Vec<ExtMessageSummary>,
) {
    fixture
        .events
        .try_send(Event::ExtMessages {
            project: fixture.project,
            query_revision,
            source_id,
            collection,
            offset,
            result: Ok(ExtMessagePage {
                total,
                next_offset: (offset + rows.len() < total).then_some(offset + rows.len()),
                rows,
            }),
        })
        .unwrap();
}

pub(in crate::workbench) fn select(fixture: &Fixture, row: usize, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click_at(
            "table",
            point(px(90.), px(theme::ROW_HEIGHT * (row as f32 + 1.5))),
            cx,
        );
    })
    .unwrap();
}

pub(in crate::workbench) fn choose_collection(
    fixture: &Fixture,
    query: &str,
    cx: &mut TestAppContext,
) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.within("ext-collection-select").click("input", cx);
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

pub(in crate::workbench) fn fragment(
    fixture: &Fixture,
    selection: (u64, Uuid, u64, usize),
    summary: ExtMessageSummary,
    bytes: Vec<u8>,
) {
    fixture
        .events
        .try_send(Event::ExtMessage {
            project: fixture.project,
            selection_revision: selection.0,
            message_id: selection.1,
            offset: selection.2,
            result: Ok(ExtMessageFragment {
                summary,
                offset: selection.2,
                bytes,
            }),
        })
        .unwrap();
}
