use super::super::tests::{click_row, message};
use super::*;
use crate::workbench::tests::{Fixture, drain, fixture};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;

mod part_1;
#[path = "save_tests.rs"]
mod save_tests;

pub(super) fn click(f: &Fixture, id: &'static str, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
    cx.run_until_parked();
}
pub(super) fn opened(cx: &mut TestAppContext) -> (Fixture, WebSocketSummary, WebSocketSummary) {
    let mut f = fixture(cx, vec![], 64);
    drain(&mut f.commands);
    click(&f, "proxy-websocket-history", cx);
    let commands = drain(&mut f.commands);
    let [Command::QueryWebSocketHistory { query_revision, .. }] = commands.as_slice() else {
        panic!("missingquery")
    };
    let first = message(Uuid::new_v4(), 1, b"payload", WebSocketKind::Text).0;
    let second = message(Uuid::new_v4(), 2, b"other", WebSocketKind::Text).0;
    f.events
        .try_send(Event::WebSocketHistoryPage {
            project: f.project,
            query_revision: *query_revision,
            flow_id: None,
            filter: WebSocketHistoryFilter::default(),
            selected_match: None,
            offset: 0,
            total: 2,
            rows: vec![first.clone(), second.clone()],
        })
        .unwrap();
    cx.run_until_parked();
    click_row(&f, 0, cx);
    drain(&mut f.commands);
    click(&f, "websocket-notes-toggle", cx);
    let commands = drain(&mut f.commands);
    let [
        Command::ReadWebSocketNotes {
            selection_revision,
            message_id,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("opening Notes must read only selected note")
    };
    assert_eq!(*message_id, first.id);
    f.events
        .try_send(Event::WebSocketNotes {
            project: f.project,
            selection_revision: *selection_revision,
            message_id: first.id,
            result: Ok(WebSocketNotes::default()),
        })
        .unwrap();
    cx.run_until_parked();
    (f, first, second)
}
pub(super) fn input(f: &Fixture, text: &str, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let editor = f.view.read(cx).websocket.notes.editor.clone();
            window.click(("input", editor.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input(text, cx);
        })
        .unwrap();
    cx.run_until_parked();
}
pub(super) fn set(f: &mut Fixture) -> (Uuid, u64, String) {
    drain(&mut f.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::SetWebSocketNotes {
                operation_id,
                expected_notes_revision,
                text,
                ..
            } => Some((operation_id, expected_notes_revision, text)),
            _ => None,
        })
        .expect("oneautosave")
}
pub(super) fn ack(
    f: &Fixture,
    id: Uuid,
    operation: Uuid,
    revision: u64,
    text: &str,
    cx: &mut TestAppContext,
) {
    f.events
        .try_send(Event::WebSocketNotesChanged {
            project: f.project,
            operation_id: operation,
            message_id: id,
            result: Ok(WebSocketNotes {
                revision,
                text: text.into(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn notes_autosave_single_inflight_retains_newer_text_and_does_not_read_payload(
    cx: &mut TestAppContext,
) {
    let (mut f, first, _) = opened(cx);
    input(&f, "第一版🙂", cx);
    let first_save = set(&mut f);
    assert_eq!(first_save.1, 0);
    assert_eq!(first_save.2, "第一版🙂");
    input(&f, "第二版\nNotes", cx);
    assert!(drain(&mut f.commands).is_empty());
    ack(&f, first.id, first_save.0, 1, &first_save.2, cx);
    let second_save = set(&mut f);
    assert_eq!(second_save.1, 1);
    assert_eq!(second_save.2, "第二版\nNotes");
    cx.read(|cx| {
        assert_eq!(
            f.view
                .read(cx)
                .websocket
                .notes
                .editor
                .read(cx)
                .value()
                .as_ref(),
            "第二版\nNotes"
        )
    });
    ack(&f, first.id, second_save.0, 2, &second_save.2, cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(!view.websocket_notes_unsaved(cx));
        assert_eq!(
            view.websocket
                .selected_summary
                .as_ref()
                .unwrap()
                .notes_revision,
            2
        );
        assert_eq!(
            view.websocket
                .selected_summary
                .as_ref()
                .unwrap()
                .notes_preview,
            "第二版\nNotes"
        );
    });
    assert!(
        drain(&mut f.commands)
            .iter()
            .all(|command| matches!(command, Command::QueryWebSocketHistory { .. }))
    );
}

#[gpui_kit::test]
fn notes_failed_save_switch_hide_and_retry_preserve_draft(cx: &mut TestAppContext) {
    let (mut f, first, second) = opened(cx);
    input(&f, "kept draft", cx);
    let save = set(&mut f);
    f.events
        .try_send(Event::WebSocketNotesChanged {
            project: f.project,
            operation_id: save.0,
            message_id: first.id,
            result: Err(WebSocketNotesError {
                message: "disk unavailable".into(),
                stored: Some(WebSocketNotes::default()),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                assert!(view.flush_websocket_notes(window, cx).is_err())
            });
        })
        .unwrap();
    click_row(&f, 1, cx);
    let commands = drain(&mut f.commands);
    let revision = commands
        .iter()
        .find_map(|command| match command {
            Command::ReadWebSocketNotes {
                message_id,
                selection_revision,
                ..
            } if *message_id == second.id => Some(*selection_revision),
            _ => None,
        })
        .unwrap();
    f.events
        .try_send(Event::WebSocketNotes {
            project: f.project,
            selection_revision: revision,
            message_id: second.id,
            result: Ok(WebSocketNotes {
                revision: 0,
                text: "other note".into(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    click_row(&f, 0, cx);
    drain(&mut f.commands);
    cx.read(|cx| {
        assert_eq!(
            f.view
                .read(cx)
                .websocket
                .notes
                .editor
                .read(cx)
                .value()
                .as_ref(),
            "kept draft"
        )
    });
    click(&f, "websocket-notes-close", cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(!view.websocket.notes.open);
        assert!(view.websocket_notes_unsaved(cx));
    });
    click(&f, "websocket-notes-toggle", cx);
    click(&f, "websocket-notes-retry", cx);
    let retry = set(&mut f);
    assert_eq!(retry.2, "kept draft");
    ack(&f, first.id, retry.0, 1, &retry.2, cx);
    cx.read(|cx| assert!(!f.view.read(cx).websocket_notes_unsaved(cx)));
}

#[gpui_kit::test]
fn notes_ime_refuses_row_switch_and_project_flush_without_losing_composition(
    cx: &mut TestAppContext,
) {
    let (mut f, first, _) = opened(cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let editor = f.view.read(cx).websocket.notes.editor.clone();
            window.click(("input", editor.entity_id()), cx);
            editor.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
            });
        })
        .unwrap();
    click_row(&f, 1, cx);
    f.window
        .update(cx, |_, window, cx| {
            let editor = f.view.read(cx).websocket.notes.editor.clone();
            assert_eq!(f.view.read(cx).websocket.selected, Some(first.id));
            assert!(editor.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            }));
            f.view.update(cx, |view, cx| {
                assert!(view.flush_websocket_notes(window, cx).is_err())
            });
        })
        .unwrap();
    assert!(drain(&mut f.commands).is_empty());
}
