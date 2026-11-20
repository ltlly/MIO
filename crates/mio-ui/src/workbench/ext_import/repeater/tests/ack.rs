use super::*;

fn browse_history_request(fixture: &mut Fixture, cx: &mut TestAppContext) -> ExtMessageSummary {
    let mut rows = browse(fixture, cx);
    for row in &mut rows {
        row.collection = "root.f2.f1[0].f2".into();
        row.role = "request".into();
        row.field_id = 2;
    }
    click(fixture, "ext-messages-refresh", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryExtMessages {
            query_revision,
            source_id,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("refresh must request the archive message index");
    };
    fixture
        .events
        .try_send(Event::ExtMessages {
            project: fixture.project,
            query_revision: *query_revision,
            source_id: *source_id,
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
    rows[0].clone()
}

mod part_1;
mod part_2;
