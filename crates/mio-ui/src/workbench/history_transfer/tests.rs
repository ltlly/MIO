use super::*;
use crate::workbench::tests::{Fixture, drain, fixture, record, tab};
use gpui_kit::test::{TestEventExt, TestWindowExt};
use gpui_kit::{MouseButton, TestAppContext};
use mio_core::{ExtHistoryRow, ExtHttpHighlight, ExtItemSummary, ExtMessageSummary};

mod lifecycle;

pub(super) fn archived(source: Uuid, index: u64) -> HistoryRow {
    let id = Uuid::new_v4();
    let message = |field_id| ExtMessageSummary {
        id: Uuid::new_v4(),
        source_id: source,
        item_id: id,
        collection: "root.f1.f0".into(),
        item_index: index,
        role: if field_id == 17 {
            "edited_request"
        } else {
            "request"
        }
        .into(),
        field_id,
        raw_offset: 0,
        raw_bytes: 50,
        body_offset: None,
        declared_version: Some(HttpVersion::Http11),
        method: Some("GET".into()),
        request_target: Some("/fixture".into()),
        status: None,
        diagnostic: None,
    };
    HistoryRow::ExtProxy(Box::new(ExtHistoryRow {
        item: ExtItemSummary {
            id,
            source_id: source,
            collection: "root.f1.f0".into(),
            index,
            object_address: None,
            state: "mapped".into(),
            message_count: 2,
        },
        request: Some(message(15)),
        edited_request: Some(message(17)),
        response: None,
        edited_response: None,
        origin: Some("http://127.0.0.1:9001".into()),
        notes_revision: 0,
        notes_preview: String::new(),
        notes_error: None,
        highlight: Some(ExtHttpHighlight {
            revision: 0,
            color: 0,
        }),
        highlight_error: None,
        time_unix_ms: None,
        time_error: None,
    }))
}

pub(super) fn setup(cx: &mut TestAppContext) -> (Fixture, Vec<HistoryRow>) {
    let mut f = fixture(cx, vec![tab("10")], 64);
    let initial = drain(&mut f.commands);
    let [
        Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery { query_revision, .. },
        },
    ] = initial.as_slice()
    else {
        panic!("initial history query")
    };
    let source = Uuid::new_v4();
    let rows = vec![
        archived(source, 0),
        archived(source, 1),
        HistoryRow::Live(record("http://127.0.0.1:9002/path?q=one", "").summary),
    ];
    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision: *query_revision,
            offset: 0,
            total: rows.len(),
            rows: rows.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("proxy-history", cx);
            window.click("table-cell-0:0", cx);
            window.click_with_modifiers("table-cell-2:0", crate::secondary_modifiers(true), cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.history.read(cx).delegate().selected_ids,
            HashSet::from([rows[0].id(), rows[2].id()])
        );
        assert_eq!(view.selected_history, Some(rows[2].id()));
    });
    drain(&mut f.commands);
    (f, rows)
}

pub(super) fn menu(f: &Fixture, intruder: bool, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.right_click("table-cell-2:0", cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let at = window
                .within("popup-menu")
                .find_by_label(if intruder {
                    "发送到 Intruder"
                } else {
                    "发送到 Repeater"
                })
                .bounds()
                .center();
            window.pointer_down(at, MouseButton::Left, cx);
            window.pointer_up(at, MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
}

pub(super) fn relevant(f: &mut Fixture) -> Vec<Command> {
    drain(&mut f.commands)
        .into_iter()
        .filter(|command| {
            // Opening Repeater also queries the active tab's navigation list.
            if let Command::QueryRepeaterHistory {
                project, tab_id, ..
            } = command
            {
                assert_eq!(*project, f.project);
                assert!(!tab_id.is_nil());
                return false;
            }
            !matches!(
                command,
                Command::QueryHttpHistory { .. }
                    | Command::ReadFlow { .. }
                    | Command::ReadExtMessage { .. }
                    | Command::ReadExtHttpNotes { .. }
            )
        })
        .collect()
}

pub(super) fn command_summary(commands: &[Command]) -> Vec<String> {
    commands
        .iter()
        .map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => format!(
                "SaveWorkspace revision={} tabs={} drafts={:?}",
                workspace.revision,
                workspace.tabs.len(),
                workspace
                    .tabs
                    .iter()
                    .map(|tab| (tab.draft.revision, tab.draft.raw.len()))
                    .collect::<Vec<_>>()
            ),
            Command::ReadHistoryTransfer {
                workspace_revision,
                entries,
                ..
            } => format!(
                "ReadHistoryTransfer revision={workspace_revision} entries={}",
                entries.len()
            ),
            Command::EnsureIntruderDefaultPool { .. } => "EnsureIntruderDefaultPool".into(),
            other => format!("{:?}", std::mem::discriminant(other)),
        })
        .collect()
}

pub(super) fn state_summary(
    f: &Fixture,
    cx: &TestAppContext,
) -> (u64, Option<u64>, bool, Vec<(u64, usize)>, bool) {
    cx.read(|cx| {
        let view = f.view.read(cx);
        (
            view.workspace.revision,
            view.pending_workspace,
            view.workspace_dirty,
            view.workspace
                .tabs
                .iter()
                .map(|tab| (tab.draft.revision, tab.draft.raw.len()))
                .collect(),
            view.error.is_some(),
        )
    })
}

pub(super) fn read(f: &mut Fixture) -> (Uuid, u64, Vec<HistoryTransferEntry>, CancellationToken) {
    let commands = relevant(f);
    let [
        Command::ReadHistoryTransfer {
            project,
            operation_id,
            workspace_revision,
            entries,
            cancel,
        },
    ] = commands.as_slice()
    else {
        panic!("one read-only batch, got {} commands", commands.len())
    };
    assert_eq!(*project, f.project);
    (
        *operation_id,
        *workspace_revision,
        entries.clone(),
        cancel.clone(),
    )
}

pub(super) fn proposals(entries: &[HistoryTransferEntry]) -> Vec<(HistoryEntryId, WorkTab)> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let mut result = tab("proposal");
            result.draft.raw =
                format!("GET /latest-{index} HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n")
                    .into_bytes();
            if matches!(entry, HistoryTransferEntry::Live { .. }) {
                result.draft.target = "http://127.0.0.1:9002/path?q=one".into();
            }
            (entry.id(), result)
        })
        .collect()
}

pub(super) fn reply(
    f: &Fixture,
    op: Uuid,
    revision: u64,
    result: Result<Vec<(HistoryEntryId, WorkTab)>, String>,
    cx: &mut TestAppContext,
) {
    f.events
        .try_send(Event::HistoryTransferRead {
            project: f.project,
            operation_id: op,
            workspace_revision: revision,
            result,
        })
        .unwrap();
    cx.run_until_parked();
}

pub(super) fn edit_active(f: &Fixture, value: &str, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("tool-repeater", cx);
            window.render_frame(cx);
            let view = f.view.read(cx);
            let editor = view.tab_editors[&view.workspace.active.unwrap()]
                .request
                .raw
                .clone();
            let before = editor.read(cx).document_revision();
            window.click(("input", editor.entity_id()), cx);
            assert!(editor.focus_handle(cx).is_focused(window));
            window.press(crate::kbd!("a"), cx);
            window.input(value, cx);
            assert_eq!(editor.read(cx).value().as_ref(), value);
            assert!(editor.read(cx).document_revision() > before);
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn repeater_multi_menu_uses_latest_visual_order_and_preserves_live_url_and_late_edits(
    cx: &mut TestAppContext,
) {
    let (mut f, rows) = setup(cx);
    menu(&f, false, cx);
    let (op, revision, entries, _) = read(&mut f);
    let HistoryRow::ExtProxy(first) = &rows[0] else {
        unreachable!()
    };
    assert!(
        matches!(entries[0], HistoryTransferEntry::External { message_id, .. } if message_id == first.edited_request.as_ref().unwrap().id)
    );
    assert_eq!(
        entries
            .iter()
            .map(HistoryTransferEntry::id)
            .collect::<Vec<_>>(),
        [rows[0].id(), rows[2].id()]
    );
    assert!(matches!(
        entries[1],
        HistoryTransferEntry::Live {
            original: false,
            ..
        }
    ));
    let latest = "GET /中文 HTTP/1.1\r\nHost: local.invalid\r\n\r\n";
    edit_active(&f, latest, cx);
    let tabs = proposals(&entries);
    reply(&f, op, revision, Ok(tabs.clone()), cx);
    let commands = relevant(&mut f);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!(
            "one atomic workspace save; commands={:?}, state={:?}",
            command_summary(&commands),
            state_summary(&f, cx)
        )
    };
    assert_eq!(workspace.tabs.len(), 3);
    assert_eq!(workspace.tabs[0].draft.raw, latest.as_bytes());
    assert_eq!(workspace.tabs[1].draft, tabs[0].1.draft);
    assert_eq!(workspace.tabs[2].draft, tabs[1].1.draft);
    assert_eq!(workspace.tabs[1].title, "11: /latest-0");
    assert_eq!(workspace.tabs[2].title, "12: /latest-1");
    assert_eq!(workspace.active, Some(tabs[1].1.id));
    let saved = workspace.clone();
    let later = "GET /later HTTP/1.1\r\nHost: latest.invalid\r\n\r\n";
    edit_active(&f, later, cx);
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: saved,
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                assert_eq!(view.workspace.tabs.len(), 3);
                assert_eq!(
                    view.tab_editors[&tabs[1].1.id]
                        .request
                        .bytes(window, cx)
                        .unwrap(),
                    later.as_bytes()
                );
                assert!(view.tab_editors[&tabs[1].1.id].dirty);
            });
        })
        .unwrap();
    assert!(relevant(&mut f).is_empty());
}

#[gpui_kit::test]
fn intruder_multi_menu_reads_first_visible_latest_request_and_creates_only_one_tab(
    cx: &mut TestAppContext,
) {
    let (mut f, rows) = setup(cx);
    menu(&f, true, cx);
    let (op, revision, entries, _) = read(&mut f);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id(), rows[0].id());
    let HistoryRow::ExtProxy(first) = &rows[0] else {
        unreachable!()
    };
    assert!(
        matches!(entries[0], HistoryTransferEntry::External { message_id, .. } if message_id == first.edited_request.as_ref().unwrap().id)
    );
    let tabs = proposals(&entries);
    reply(&f, op, revision, Ok(tabs.clone()), cx);
    let commands = relevant(&mut f);
    let [Command::EnsureIntruderDefaultPool { operation_id, .. }] = commands.as_slice() else {
        panic!("one default pool admission; no attack or tabs yet")
    };
    cx.read(|cx| assert!(f.view.read(cx).workspace.intruder.tabs.is_empty()));
    let pool = Uuid::new_v4();
    f.events
        .try_send(Event::IntruderDefaultPoolReady {
            project: f.project,
            operation_id: *operation_id,
            result: Ok(mio_core::IntruderResourcePools {
                revision: 1,
                default_pool_id: Some(pool),
                pools: vec![mio_core::IntruderResourcePool {
                    retained_inputs: None,
                    id: pool,
                    name: "Default resource pool".into(),
                    max_concurrent_requests: Some(10),
                    delay: None,
                    automatic_throttling: mio_core::IntruderAutoThrottle {
                        enabled: true,
                        ..Default::default()
                    },
                }],
            }),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = relevant(&mut f);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("one saved tab, no Start")
    };
    assert_eq!(workspace.intruder.tabs.len(), 1);
    assert_eq!(workspace.intruder.tabs[0].spec.draft, tabs[0].1.draft);
    assert_eq!(
        workspace.intruder.active,
        Some(workspace.intruder.tabs[0].id)
    );
}
