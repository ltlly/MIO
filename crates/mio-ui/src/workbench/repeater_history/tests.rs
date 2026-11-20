use super::*;
use crate::workbench::tests::{Fixture, drain, fixture, tab};
use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{EntityInputHandler, TestAppContext};
use mio_core::{
    RepeaterHistory, RepeaterHistoryCursor, RepeaterHistoryEntry, RepeaterHistorySource, Uuid,
};

mod part_1;
mod part_2;

fn history_tab() -> WorkTab {
    let mut tab = tab("同一历史标签");
    tab.history = Some(RepeaterHistory {
        source: Some(RepeaterHistorySource {
            source_id: Uuid::new_v4(),
            parent_item_id: Uuid::new_v4(),
        }),
        cursor: Some(RepeaterHistoryCursor {
            entry: RepeaterHistoryEntryId::Archive(2),
            draft_revision: 1,
        }),
    });
    tab
}
pub(super) fn click(f: &Fixture, id: &'static str, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
    cx.run_until_parked();
}
pub(super) fn query(f: &mut Fixture) -> (u64, u64) {
    let commands = drain(&mut f.commands);
    let [
        Command::QueryRepeaterHistory {
            query_revision,
            start,
            limit: HISTORY_PAGE_SIZE,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one history query expected")
    };
    (*query_revision, *start)
}
pub(super) fn page(
    f: &Fixture,
    query: (u64, u64),
    current: u64,
    total: u64,
    cx: &mut TestAppContext,
) {
    f.events
        .try_send(Event::RepeaterHistoryPage {
            project: f.project,
            query_revision: query.0,
            tab_id: cx.read(|cx| f.view.read(cx).workspace.active.unwrap()),
            result: Ok(RepeaterHistoryPage {
                total,
                start: query.1,
                current_position: Some(current),
                rows: (query.1..(query.1 + HISTORY_PAGE_SIZE as u64).min(total))
                    .map(|position| RepeaterHistoryEntry {
                        id: RepeaterHistoryEntryId::Archive(position),
                        position,
                        method: Some("GET".into()),
                        target: Some(format!("http://127.0.0.1:19081/history/{position}")),
                        status: Some(200),
                        diagnostic: None,
                    })
                    .collect(),
                next_offset: (query.1 + (HISTORY_PAGE_SIZE as u64) < total)
                    .then_some(query.1 + HISTORY_PAGE_SIZE as u64),
            }),
        })
        .unwrap();
    cx.run_until_parked();
}
pub(super) fn boot(cx: &mut TestAppContext) -> Fixture {
    let mut f = fixture(cx, vec![history_tab()], 64);
    drain(&mut f.commands);
    click(&f, "tool-repeater", cx);
    let q = query(&mut f);
    page(&f, q, 2, 4, cx);
    f
}
pub(super) fn select(f: &mut Fixture) -> (Uuid, RepeaterHistoryEntryId, u64, u64) {
    let commands = drain(&mut f.commands);
    let [
        Command::SelectRepeaterHistory {
            operation_id,
            entry,
            workspace_revision,
            draft_revision,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one same-tab selection expected")
    };
    (*operation_id, *entry, *workspace_revision, *draft_revision)
}
pub(super) fn proposal(
    f: &Fixture,
    selection: (Uuid, RepeaterHistoryEntryId, u64, u64),
    cx: &mut TestAppContext,
) -> WorkTab {
    let mut tab = cx.read(|cx| f.view.read(cx).workspace.tabs[0].clone());
    tab.draft.revision = selection.3 + 1;
    tab.draft.raw = b"GET /history/1 HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec();
    tab.history.as_mut().unwrap().cursor = Some(RepeaterHistoryCursor {
        entry: selection.1,
        draft_revision: tab.draft.revision,
    });
    tab
}
pub(super) fn selected(
    f: &Fixture,
    selection: (Uuid, RepeaterHistoryEntryId, u64, u64),
    tab: WorkTab,
    cx: &mut TestAppContext,
) {
    f.events
        .try_send(Event::RepeaterHistorySelected {
            project: f.project,
            operation_id: selection.0,
            tab_id: tab.id,
            entry: selection.1,
            workspace_revision: selection.2,
            draft_revision: selection.3,
            result: Ok(tab),
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn history_previous_saves_edited_draft_then_applies_same_tab_without_send(cx: &mut TestAppContext) {
    let mut f = boot(cx);
    let id = cx.read(|cx| f.view.read(cx).workspace.active.unwrap());
    f.window
        .update(cx, |_, window, cx| {
            let editor = f.view.read(cx).tab_editors[&id].request.raw.clone();
            window.click(("input", editor.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input("GET /edited HTTP/1.1\r\n\r\n", cx);
        })
        .unwrap();
    click(&f, "repeater-history-previous", cx);
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("history navigation waits for draft save")
    };
    assert_eq!(workspace.tabs[0].draft.raw, b"GET /edited HTTP/1.1\r\n\r\n");
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let choice = select(&mut f);
    assert_eq!(choice.1, RepeaterHistoryEntryId::Archive(1));
    let next = proposal(&f, choice, cx);
    selected(&f, choice, next.clone(), cx);
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("applying navigation saves only workspace")
    };
    assert_eq!(workspace.tabs, [next]);
    assert_eq!(workspace.active, Some(id));
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.workspace.tabs.len(), 1);
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert_eq!(
            view.repeater_history
                .page
                .as_ref()
                .unwrap()
                .current_position,
            Some(1)
        );
    });
}

#[gpui_kit::test]
fn history_late_selection_and_ime_never_replace_newer_editor_content(cx: &mut TestAppContext) {
    let mut f = boot(cx);
    let id = cx.read(|cx| f.view.read(cx).workspace.active.unwrap());
    click(&f, "repeater-history-previous", cx);
    let choice = select(&mut f);
    let next = proposal(&f, choice, cx);
    f.window
        .update(cx, |_, window, cx| {
            let editor = f.view.read(cx).tab_editors[&id].request.raw.clone();
            window.click(("input", editor.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input("GET /newer HTTP/1.1\r\n\r\n", cx);
        })
        .unwrap();
    selected(&f, choice, next, cx);
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.workspace.tabs[0].draft.raw,
            b"GET /newer HTTP/1.1\r\n\r\n"
        );
        assert!(view.workspace_dirty);
        assert!(view.repeater_history.error.is_some());
    });
    f.window
        .update(cx, |_, window, cx| {
            let editor = f.view.read(cx).tab_editors[&id].request.raw.clone();
            editor.update(cx, |editor, cx| {
                editor.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
            });
            window.click("repeater-history-next", cx);
            assert!(editor.update(cx, |editor, cx| {
                editor.marked_text_range(window, cx).is_some()
            }));
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn history_previous_reads_only_nearby_page_for_off_page_current_position(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![history_tab()], 64);
    drain(&mut f.commands);
    click(&f, "tool-repeater", cx);
    let initial = query(&mut f);
    page(&f, initial, 100, 150, cx);
    click(&f, "repeater-history-previous", cx);
    let nearby = query(&mut f);
    assert_eq!(nearby.1, 99 - HISTORY_PAGE_SIZE as u64 / 2);
    page(&f, nearby, 100, 150, cx);
    let selection = select(&mut f);
    assert_eq!(selection.1, RepeaterHistoryEntryId::Archive(99));
    cx.read(|cx| {
        assert!(
            f.view
                .read(cx)
                .repeater_history
                .page
                .as_ref()
                .unwrap()
                .rows
                .len()
                <= HISTORY_PAGE_SIZE
        )
    });
}

pub(super) fn sending(f: &mut Fixture, cx: &mut TestAppContext) -> (Uuid, Draft) {
    click(f, "repeater-send", cx);
    let commands = drain(&mut f.commands);
    let [Command::Send { run_id, draft, .. }] = commands.as_slice() else {
        panic!("only the explicit Send may start a run")
    };
    let sent = (*run_id, draft.clone());
    f.events
        .try_send(Event::SendStarted {
            project: f.project,
            tab_id: cx.read(|cx| f.view.read(cx).workspace.active.unwrap()),
            run_id: sent.0,
            draft_revision: sent.1.revision,
        })
        .unwrap();
    cx.run_until_parked();
    let refreshed = query(f);
    page(f, refreshed, 2, 4, cx);
    sent
}

pub(super) fn finish(f: &Fixture, sent: &(Uuid, Draft), cx: &mut TestAppContext) -> FlowRecord {
    let mut record = crate::workbench::tests::record(&sent.1.target, "live response");
    record.summary.id = sent.0;
    record.summary.state = mio_core::FlowState::Complete;
    record.response = Some(
        parse_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nlive",
            HttpVersion::Http11,
        )
        .unwrap(),
    );
    f.events
        .try_send(Event::SendFinished {
            project: f.project,
            tab_id: cx.read(|cx| f.view.read(cx).workspace.active.unwrap()),
            draft_revision: sent.1.revision,
            record: Box::new(record.clone()),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
    record
}
