use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, point};

use crate::workbench::tests::{Fixture, drain, fixture, record, tab};
use crate::workbench::*;

fn emit(fixture: &Fixture, event: Event, cx: &mut TestAppContext) {
    fixture.events.try_send(event).unwrap();
    cx.run_until_parked();
}

fn fail(
    fixture: &Fixture,
    project: ProjectKey,
    revision: u64,
    flow_id: FlowId,
    message: &str,
    cx: &mut TestAppContext,
) {
    emit(
        fixture,
        Event::FlowDetailError {
            project,
            selection_revision: revision,
            flow_id,
            message: message.into(),
        },
        cx,
    );
}

fn history_read(fixture: &mut Fixture) -> (u64, FlowId) {
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadFlow {
            selection_revision,
            flow_id,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one detail read must follow a real history selection")
    };
    (*selection_revision, *flow_id)
}

#[gpui_kit::test]
fn history_error_matches_current_click_and_the_retry_button_reads_again(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let a = record("http://127.0.0.1:1/a", "first");
    let b = record("http://127.0.0.1:1/b", "second");
    let query = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery { query_revision, .. },
            } => Some(query_revision),
            _ => None,
        })
        .expect("fixture starts the history read");
    emit(
        &fixture,
        Event::HistoryPage {
            project: fixture.project,
            query_revision: query,
            offset: 0,
            total: 2,
            rows: vec![a.summary.clone().into(), b.summary.clone().into()],
        },
        cx,
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-history", cx);
        window.render_frame(cx);
        window.click_at("table", point(px(90.), px(theme::ROW_HEIGHT * 1.5)), cx);
    })
    .unwrap();
    let (first, first_id) = history_read(&mut fixture);
    assert_eq!(first_id, a.summary.id);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click_at("table", point(px(90.), px(theme::ROW_HEIGHT * 2.5)), cx);
    })
    .unwrap();
    let (selected, selected_id) = history_read(&mut fixture);
    assert!(selected > first);
    assert_eq!(selected_id, b.summary.id);
    for (project, revision, flow_id) in [
        (fixture.project, first, a.summary.id),
        (
            ProjectKey {
                generation: fixture.project.generation + 1,
                ..fixture.project
            },
            selected,
            b.summary.id,
        ),
        (fixture.project, selected, a.summary.id),
    ] {
        fail(
            &fixture,
            project,
            revision,
            flow_id,
            "unrelated failure",
            cx,
        );
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.selected_flow, Some(b.summary.id));
            assert!(view.detail.is_none());
            assert!(view.detail_error.is_none());
            assert!(view.error.is_none());
        });
    }
    fail(
        &fixture,
        fixture.project,
        selected,
        b.summary.id,
        "current read failed",
        cx,
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert_eq!(view.detail_error.as_deref(), Some("current read failed"));
        assert!(window.find("history-retry").bounds().size.width > px(0.));
        window.click("history-retry", cx);
    })
    .unwrap();
    let (retry, retry_id) = history_read(&mut fixture);
    assert!(retry > selected);
    assert_eq!(retry_id, b.summary.id);
    fail(
        &fixture,
        fixture.project,
        selected,
        b.summary.id,
        "late original failure",
        cx,
    );
    cx.read(|cx| assert!(fixture.view.read(cx).detail_error.is_none()));
    emit(
        &fixture,
        Event::FlowDetail {
            project: fixture.project,
            selection_revision: retry,
            record: Box::new(b.clone()),
            original: Box::default(),
        },
        cx,
    );
    fail(
        &fixture,
        fixture.project,
        retry,
        b.summary.id,
        "duplicate failure after success",
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.detail.as_deref(), Some(&b));
        assert!(view.detail_error.is_none());
        assert!(view.error.is_none());
    });
}

#[gpui_kit::test]
fn wrong_success_does_not_consume_a_saved_repeater_read(cx: &mut TestAppContext) {
    let mut saved = tab("Saved");
    let mut expected = record("http://127.0.0.1:1/expected", "request");
    expected.response = Some(
        parse_response(
            b"HTTP/1.1 200 OK\r\n\r\nexpected response",
            HttpVersion::Http11,
        )
        .unwrap(),
    );
    saved.last_flow = Some(expected.summary.id);
    let mut fixture = fixture(cx, vec![saved.clone()], 64);
    let revision = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::ReadFlow {
                selection_revision,
                flow_id,
                ..
            } if flow_id == expected.summary.id => Some(selection_revision),
            _ => None,
        })
        .unwrap();
    emit(
        &fixture,
        Event::FlowDetail {
            project: fixture.project,
            selection_revision: revision,
            record: Box::new(record("http://127.0.0.1:1/wrong", "other request")),
            original: Box::default(),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.repeater_reads.get(&revision), Some(&saved.id));
        assert!(
            view.tab_editors[&saved.id]
                .response
                .raw
                .read(cx)
                .value()
                .is_empty()
        );
        assert!(view.tab_editors[&saved.id].result.is_none());
    });
    emit(
        &fixture,
        Event::FlowDetail {
            project: fixture.project,
            selection_revision: revision,
            record: Box::new(expected),
            original: Box::default(),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.repeater_reads.contains_key(&revision));
        assert!(
            view.tab_editors[&saved.id]
                .response
                .raw
                .read(cx)
                .value()
                .contains("expected response")
        );
    });
}

#[gpui_kit::test]
fn restored_repeater_read_errors_do_not_consume_other_tabs_or_completed_reads(
    cx: &mut TestAppContext,
) {
    let mut a = tab("A");
    let mut b = tab("B");
    let a_flow = record("http://127.0.0.1:1/a", "first");
    let mut b_flow = record("http://127.0.0.1:1/b", "second");
    b_flow.response =
        Some(parse_response(b"HTTP/1.1 200 OK\r\n\r\nresponse B", HttpVersion::Http11).unwrap());
    a.last_flow = Some(a_flow.summary.id);
    b.last_flow = Some(b_flow.summary.id);
    let mut fixture = fixture(cx, vec![a.clone(), b.clone()], 64);
    fixture
        .window
        .update(cx, |_, window, cx| {
            fixture.view.update(cx, |view, cx| {
                view.ensure_tab_editor(b.id, window, cx);
            });
        })
        .unwrap();
    let reads: HashMap<_, _> = drain(&mut fixture.commands)
        .into_iter()
        .filter_map(|command| match command {
            Command::ReadFlow {
                selection_revision,
                flow_id,
                ..
            } => Some((flow_id, selection_revision)),
            _ => None,
        })
        .collect();
    let a_read = reads[&a_flow.summary.id];
    let b_read = reads[&b_flow.summary.id];
    fail(
        &fixture,
        fixture.project,
        a_read,
        b_flow.summary.id,
        "wrong tab",
        cx,
    );
    fail(
        &fixture,
        ProjectKey {
            generation: fixture.project.generation + 1,
            ..fixture.project
        },
        a_read,
        a_flow.summary.id,
        "old project",
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.repeater_reads.len(), 2);
        assert!(view.tab_editors[&a.id].result.is_none());
        assert!(view.tab_editors[&b.id].result.is_none());
        assert!(view.error.is_none());
    });
    fail(
        &fixture,
        fixture.project,
        a_read,
        a_flow.summary.id,
        "read A failed",
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.repeater_reads.contains_key(&a_read));
        assert_eq!(view.repeater_reads.get(&b_read), Some(&b.id));
        assert!(
            view.tab_editors[&a.id]
                .result
                .as_ref()
                .unwrap()
                .contains("read A failed")
        );
        assert!(view.tab_editors[&b.id].result.is_none());
    });
    emit(
        &fixture,
        Event::FlowDetail {
            project: fixture.project,
            selection_revision: b_read,
            record: Box::new(b_flow.clone()),
            original: Box::default(),
        },
        cx,
    );
    let before = cx.read(|cx| {
        let view = fixture.view.read(cx);
        (
            view.tab_editors[&b.id].response.raw.read(cx).value(),
            view.tab_editors[&b.id].result.clone(),
            view.error.clone(),
        )
    });
    fail(
        &fixture,
        fixture.project,
        a_read,
        a_flow.summary.id,
        "duplicate A failure",
        cx,
    );
    fail(
        &fixture,
        fixture.project,
        b_read,
        b_flow.summary.id,
        "late B failure",
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.repeater_reads.is_empty());
        assert_eq!(
            view.tab_editors[&b.id].response.raw.read(cx).value(),
            before.0
        );
        assert!(before.0.ends_with("response B"));
        assert_eq!(view.tab_editors[&b.id].result, before.1);
        assert_eq!(view.error, before.2);
        assert_eq!(view.workspace.tabs, vec![a, b]);
    });
}

#[gpui_kit::test]
fn a_repeater_tab_with_a_new_last_flow_ignores_its_previous_read_error(cx: &mut TestAppContext) {
    let mut tab = tab("changed last flow");
    let original = record("http://127.0.0.1:1/original", "old");
    let latest = record("http://127.0.0.1:1/latest", "new");
    tab.last_flow = Some(original.summary.id);
    let mut fixture = fixture(cx, vec![tab.clone()], 64);
    let read = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::ReadFlow {
                selection_revision, ..
            } => Some(selection_revision),
            _ => None,
        })
        .unwrap();
    fixture.view.update(cx, |view, _| {
        view.workspace.tabs[0].last_flow = Some(latest.summary.id);
        view.tab_editors.get_mut(&tab.id).unwrap().result = Some("new operation result".into());
    });
    fail(
        &fixture,
        fixture.project,
        read,
        original.summary.id,
        "old operation failure",
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.repeater_reads.get(&read), Some(&tab.id));
        assert_eq!(
            view.tab_editors[&tab.id].result.as_deref(),
            Some("new operation result")
        );
        assert!(view.error.is_none());
    });
}
