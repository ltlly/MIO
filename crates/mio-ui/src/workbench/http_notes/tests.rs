use super::*;
use crate::workbench::tests::{Fixture, drain, fixture};
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{TestAppContext, point};
use mio_core::ExtItemSummary;

fn row(source: Uuid, item: Uuid, index: u64) -> HistoryRow {
    HistoryRow::ExtProxy(Box::new(mio_core::ExtHistoryRow {
        item: ExtItemSummary {
            id: item,
            source_id: source,
            collection: "root.f1.f0".into(),
            index,
            object_address: None,
            state: "mapped".into(),
            message_count: 0,
        },
        request: None,
        edited_request: None,
        response: None,
        edited_response: None,
        origin: None,
        notes_revision: 0,
        notes_preview: String::new(),
        notes_error: None,
        highlight: Some(mio_core::ExtHttpHighlight {
            revision: 0,
            color: 0,
        }),
        highlight_error: None,
        time_unix_ms: None,
        time_error: None,
    }))
}
fn click(f: &Fixture, id: &str, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(SharedString::from(id.to_owned()), cx);
        })
        .unwrap();
    cx.run_until_parked();
}
fn select_row(f: &Fixture, row: usize, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click_at(
                "table",
                point(px(90.), px(theme::ROW_HEIGHT * (row as f32 + 1.5))),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
}
fn fixture_notes(cx: &mut TestAppContext) -> (Fixture, (Uuid, Uuid), (Uuid, Uuid)) {
    let mut f = fixture(cx, vec![], 64);
    let commands = drain(&mut f.commands);
    let [
        Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery { query_revision, .. },
        },
    ] = commands.as_slice()
    else {
        panic!("history query")
    };
    let source = Uuid::new_v4();
    let first = (source, Uuid::new_v4());
    let second = (source, Uuid::new_v4());
    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision: *query_revision,
            offset: 0,
            total: 2,
            rows: vec![row(first.0, first.1, 1), row(second.0, second.1, 0)],
        })
        .unwrap();
    cx.run_until_parked();
    click(&f, "proxy-history", cx);
    select_row(&f, 0, cx);
    assert!(drain(&mut f.commands).is_empty());
    (f, first, second)
}
fn read(f: &mut Fixture) -> (u64, (Uuid, Uuid)) {
    let commands = drain(&mut f.commands);
    let [
        Command::ReadExtHttpNotes {
            selection_revision,
            source_id,
            item_id,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one selected note read")
    };
    (*selection_revision, (*source_id, *item_id))
}
fn loaded(f: &Fixture, read: (u64, (Uuid, Uuid)), text: &str, cx: &mut TestAppContext) {
    f.events
        .try_send(Event::ExtHttpNotes {
            project: f.project,
            selection_revision: read.0,
            source_id: read.1.0,
            item_id: read.1.1,
            result: Ok(ExtHttpNotes {
                revision: 0,
                text: text.into(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
}
fn input(f: &Fixture, text: &str, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let editor = f.view.read(cx).http_notes.editor.clone();
            window.click(("input", editor.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            if text.is_empty() {
                window.press("backspace", cx);
            } else {
                window.input(text, cx);
            }
        })
        .unwrap();
    cx.run_until_parked();
}
fn write(f: &mut Fixture) -> (Uuid, ExtHttpNotesChange) {
    drain(&mut f.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::SetExtHttpNotes {
                operation_id,
                change,
                ..
            } => Some((operation_id, change)),
            _ => None,
        })
        .expect("autosave")
}
fn ack(
    f: &Fixture,
    operation: Uuid,
    change: &ExtHttpNotesChange,
    revision: u64,
    cx: &mut TestAppContext,
) {
    f.events
        .try_send(Event::ExtHttpNotesChanged {
            project: f.project,
            operation_id: operation,
            source_id: change.source_id,
            item_id: change.item_id,
            result: Ok(ExtHttpNotes {
                revision,
                text: change.text.clone(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn imported_http_notes_preserve_markdown_source_and_single_inflight_newer_edit(
    cx: &mut TestAppContext,
) {
    let (mut f, first, _) = fixture_notes(cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.right_click("table-cell-0:0", cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let add_notes = window.find_by_label("添加备注").bounds().center();
            window.pointer_down(add_notes, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(add_notes, gpui_kit::MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let selected = read(&mut f);
    assert_eq!(selected.1, first);
    loaded(
        &f,
        selected,
        "**source**\n![x](https://example.invalid/image)",
        cx,
    );
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).http_notes.editor.read(cx).value().as_ref(),
            "**source**\n![x](https://example.invalid/image)"
        )
    });
    assert!(drain(&mut f.commands).is_empty());
    input(&f, "**first**", cx);
    let a = write(&mut f);
    input(&f, "**second**\n中文🙂", cx);
    assert!(drain(&mut f.commands).is_empty());
    ack(&f, a.0, &a.1, 1, cx);
    let b = write(&mut f);
    assert_eq!(b.1.expected_notes_revision, 1);
    assert_eq!(b.1.text, "**second**\n中文🙂");
    ack(&f, b.0, &b.1, 2, cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let view = f.view.read(cx);
            assert!(!view.http_notes_unsaved(cx));
            assert_eq!(view.http_notes.editor.read(cx).value().as_ref(), b.1.text);
            let HistoryRow::ExtProxy(row) = &view.history.read(cx).delegate().rows[0] else {
                unreachable!()
            };
            assert_eq!(row.notes_revision, 2);
            assert_eq!(row.notes_preview, b.1.text);
            assert_eq!(
                window.find("table-cell-0:11").label(),
                Some("**second** 中文🙂")
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn imported_http_notes_read_error_retry_ime_and_row_switch_preserve_correct_owner(
    cx: &mut TestAppContext,
) {
    let (mut f, first, second) = fixture_notes(cx);
    click(&f, "http-notes-toggle", cx);
    let failed = read(&mut f);
    f.events
        .try_send(Event::ExtHttpNotes {
            project: f.project,
            selection_revision: failed.0,
            source_id: first.0,
            item_id: first.1,
            result: Err("annotation source unavailable".into()),
        })
        .unwrap();
    cx.run_until_parked();
    click(&f, "http-notes-retry", cx);
    let retry = read(&mut f);
    assert!(retry.0 > failed.0);
    loaded(&f, retry, "first", cx);
    f.window
        .update(cx, |_, window, cx| {
            let editor = f.view.read(cx).http_notes.editor.clone();
            window.click(("input", editor.entity_id()), cx);
            editor.update(cx, |editor, cx| {
                editor.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
            });
        })
        .unwrap();
    select_row(&f, 1, cx);
    f.window
        .update(cx, |_, window, cx| {
            let editor = f.view.read(cx).http_notes.editor.clone();
            assert_eq!(
                f.view.read(cx).selected_history,
                Some(HistoryEntryId::External {
                    source_id: first.0,
                    item_id: first.1
                })
            );
            assert!(editor.update(cx, |editor, cx| {
                editor.marked_text_range(window, cx).is_some()
            }));
            editor.update(cx, |editor, cx| {
                editor.unmark_text(window, cx);
                editor.set_value("first", window, cx);
            });
        })
        .unwrap();
    assert!(drain(&mut f.commands).is_empty());
    select_row(&f, 1, cx);
    let current = read(&mut f);
    assert_eq!(current.1, second);
    loaded(&f, retry, "late wrong", cx);
    loaded(&f, current, "second", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).http_notes.editor.read(cx).value().as_ref(),
            "second"
        )
    });
}

#[gpui_kit::test]
fn failed_http_notes_save_keeps_draft_for_atomic_save_as_and_rebases_only_submitted_text(
    cx: &mut TestAppContext,
) {
    let (mut f, first, second) = fixture_notes(cx);
    click(&f, "http-notes-toggle", cx);
    let r = read(&mut f);
    loaded(&f, r, "", cx);
    input(&f, "recover", cx);
    let failed = write(&mut f);
    f.events
        .try_send(Event::ExtHttpNotesChanged {
            project: f.project,
            operation_id: failed.0,
            source_id: first.0,
            item_id: first.1,
            result: Err(ExtHttpNotesError {
                message: "source read only".into(),
                stored: Some(ExtHttpNotes::default()),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    let batch = f
        .window
        .update(cx, |_, window, cx| {
            f.view
                .update(cx, |view, cx| view.snapshot_http_notes(window, cx).unwrap())
        })
        .unwrap();
    assert_eq!(batch.as_slice(), std::slice::from_ref(&failed.1));
    f.view.update(cx, |view, _| {
        view.project_http_notes_save = Some(batch.clone());
        view.project_saves_in_flight = 1;
    });
    input(&f, "", cx);
    select_row(&f, 1, cx);
    let second_read = read(&mut f);
    assert_eq!(second_read.1, second);
    loaded(&f, second_read, "other", cx);
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.project_http_notes_save = None;
                view.project_saves_in_flight = 0;
                view.accept_saved_http_notes(&batch, &[(first.0, first.1, 1)], window, cx)
                    .unwrap();
                assert!(view.http_notes_unsaved(cx));
                assert_eq!(view.http_notes.drafts[&first].text, "");
                assert_eq!(view.http_notes.drafts[&first].stored.revision, 1);
                assert_eq!(view.http_notes.editor.read(cx).value().as_ref(), "other");
            })
        })
        .unwrap();
}

#[gpui_kit::test]
fn http_notes_save_before_quit_retains_equal_text_ime_on_project_receipt(cx: &mut TestAppContext) {
    use crate::workbench::tests::pump_native;
    let (mut f, first, _) = fixture_notes(cx);
    click(&f, "http-notes-toggle", cx);
    let selection = read(&mut f);
    loaded(&f, selection, "", cx);
    let path = PathBuf::from("/tmp/mio-http-notes-quit-ime.mio");
    f.view
        .update(cx, |view, _| view.project_path = Some(path.clone()));
    let text = "same pending HTTP note";
    input(&f, text, cx);
    let save = write(&mut f);
    f.events
        .try_send(Event::ExtHttpNotesChanged {
            project: f.project,
            operation_id: save.0,
            source_id: first.0,
            item_id: first.1,
            result: Err(ExtHttpNotesError {
                message: "source unavailable".into(),
                stored: Some(ExtHttpNotes::default()),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut f.commands);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.press(crate::kbd!("q"), cx);
        })
        .unwrap();
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("保存");
    pump_native(cx);
    assert!(
        matches!(drain(&mut f.commands).as_slice(),[Command::Quiesce{project}]if *project==f.project)
    );
    f.events
        .try_send(Event::Quiesced { project: f.project })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::SaveProject {
            websocket_notes,
            ext_http_notes,
            path: None,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("Quit save must carry the failed HTTP note in an atomic project batch")
    };
    assert!(websocket_notes.is_empty());
    assert_eq!(ext_http_notes, &[save.1]);
    f.window
        .update(cx, |_, window, cx| {
            let editor = f.view.read(cx).http_notes.editor.clone();
            editor.update(cx, |editor, cx| {
                editor.replace_and_mark_text_in_range(
                    Some(0..text.len()),
                    text,
                    Some(text.len()..text.len()),
                    window,
                    cx,
                );
                assert_eq!(editor.value().as_ref(), text);
                assert!(editor.marked_text_range(window, cx).is_some());
            });
        })
        .unwrap();
    f.events
        .try_send(Event::ProjectSaved {
            project: f.project,
            path,
            total_bytes: 4096,
            websocket_notes: vec![],
            ext_http_notes: vec![(first.0, first.1, 1)],
            ext_http_highlights: vec![],
        })
        .unwrap();
    cx.run_until_parked();
    pump_native(cx);
    f.window
        .update(cx, |_, window, cx| {
            let view = f.view.read(cx);
            assert!(!view.shutdown.is_cancelled());
            assert!(!view.shutdown_requested && !view.quitting);
            assert!(view.transition.is_none());
            assert!(view.after_save.is_none());
            let editor = view.http_notes.editor.clone();
            editor.update(cx, |editor, cx| {
                assert_eq!(editor.value().as_ref(), text);
                assert!(editor.marked_text_range(window, cx).is_some());
            });
        })
        .unwrap();
    assert!(drain(&mut f.commands).iter().all(|command| matches!(
        command,
        Command::Resume { .. } | Command::QueryHttpHistory { .. }
    )));
}

#[gpui_kit::test]
fn selected_http_notes_supports_intercept_target_logger_and_dashboard_finding_flows(
    cx: &mut TestAppContext,
) {
    use crate::workbench::tests::{record, tab};
    let mut f = fixture(cx, vec![tab("1")], 64);
    drain(&mut f.commands);
    let rec = record("http://127.0.0.1:9001/intercept-note", "body");
    let intercept_flow = rec.summary.id;
    f.events
        .try_send(Event::InterceptPending {
            project: f.project,
            stage: InterceptStage::Request,
            response_head_only: false,
            record: Box::new(rec),
        })
        .unwrap();
    cx.run_until_parked();

    f.window
        .update(cx, |_, window, cx| {
            window.click("tool-proxy", cx);
            window.click("proxy-intercept", cx);
            window.render_frame(cx);
            assert_eq!(
                f.view.read(cx).selected_http_notes(),
                Some((Uuid::nil(), intercept_flow))
            );
            assert!(window.find("http-notes-toggle").visible());

            let target_flow = Uuid::new_v4();
            let logger_flow = Uuid::new_v4();
            let dash_flow = Uuid::new_v4();
            let finding_id = Uuid::new_v4();
            f.view.update(cx, |view, _| {
                view.tool = Tool::Target;
                view.target_pane.page = target::TargetPage::SiteMap;
                view.target_pane.selected_id = Some(HistoryEntryId::Live(target_flow));
                assert_eq!(
                    view.selected_http_notes(),
                    Some((Uuid::nil(), target_flow))
                );

                view.tool = Tool::Logger;
                view.extra_tools.logger_selected = Some(logger_flow);
                assert_eq!(
                    view.selected_http_notes(),
                    Some((Uuid::nil(), logger_flow))
                );

                view.tool = Tool::Dashboard;
                view.audit_dashboard.new_scan_open = false;
                view.audit_dashboard
                    .snapshot
                    .findings
                    .push(mio_core::AuditFinding {
                        id: finding_id,
                        issue_id: "NOTE-1".into(),
                        title: "Flow issue".into(),
                        severity_label: "High".into(),
                        severity_rank: 3,
                        confidence: mio_core::IssueConfidence::Certain,
                        phase: mio_core::AuditPhase::Passive,
                        host: "127.0.0.1".into(),
                        url: "http://127.0.0.1:9001/".into(),
                        parameter: None,
                        flow_id: Some(dash_flow),
                        summary: String::new(),
                        evidence: String::new(),
                        request_highlights: Vec::new(),
                        response_highlights: Vec::new(),
                        remediation: String::new(),
                        detected_unix_ms: 0,
                    });
                view.audit_dashboard.selected_finding_id = Some(finding_id);
                assert_eq!(view.selected_http_notes(), Some((Uuid::nil(), dash_flow)));
            });
        })
        .unwrap();
}

#[path = "shared_tests.rs"]
mod shared_tests;
