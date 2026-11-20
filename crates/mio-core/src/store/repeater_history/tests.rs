use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

const RAW: &[u8] = b"POST /live HTTP/1.1\r\nHost: header.invalid\r\nX-A:first\r\nX-A:\tsecond\r\nContent-Length: 2\r\n\r\n\0\xff";

fn setup() -> (ProjectStore, TabId) {
    let mut store = ProjectStore::create(None).unwrap();
    let mut workspace = store.workspace().unwrap();
    let id = workspace.open_request(None).unwrap();
    workspace.tabs[0].draft.raw = RAW.to_vec();
    workspace.tabs[0].draft.target = "http://127.0.0.1:18765".into();
    store.save_workspace(&workspace).unwrap();
    (store, id)
}

fn pending(draft: &Draft) -> FlowRecord {
    let request = crate::parse_request(&draft.raw, &draft.target, draft.version).unwrap();
    FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Repeater,
            method: request.method.clone(),
            url: request.url.clone(),
            status: None,
            state: FlowState::Pending,
            request_bytes: request.body.len() as u64,
            response_bytes: 0,
            started_unix_ms: 17,
            elapsed_ms: 0,
        },
        request,
        response: None,
    }
}

fn begin(store: &mut ProjectStore, tab: TabId) -> FlowRecord {
    let draft = store
        .workspace()
        .unwrap()
        .tabs
        .iter()
        .find(|value| value.id == tab)
        .unwrap()
        .draft
        .clone();
    let record = pending(&draft);
    store
        .begin_repeater_send(tab, &draft, &record, &CancellationToken::new())
        .unwrap();
    record
}

fn select(store: &mut ProjectStore, tab: TabId, entry: RepeaterHistoryEntryId) -> WorkTab {
    let mut workspace = store.workspace().unwrap();
    let index = workspace
        .tabs
        .iter()
        .position(|value| value.id == tab)
        .unwrap();
    let selected = store
        .load_repeater_history(
            tab,
            entry,
            workspace.revision,
            workspace.tabs[index].draft.revision,
            &CancellationToken::new(),
        )
        .unwrap();
    workspace.tabs[index] = selected.clone();
    workspace.revision += 1;
    store.save_workspace(&workspace).unwrap();
    selected
}

#[test]
fn live_inserts_after_selection_preserves_successors_and_original_draft_through_save_as() {
    let (mut store, tab) = setup();
    let first = begin(&mut store, tab);
    let first_id = RepeaterHistoryEntryId::Live(first.summary.id);
    store
        .connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "repeater_history",
                column_name: "raw" | "draft_meta",
            }
            | AuthAction::Read {
                table_name: "messages",
                column_name: "body",
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));
    assert_eq!(store.repeater_history_page(tab, 0, 64).unwrap().total, 1);
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    select(&mut store, tab, first_id);
    let second = begin(&mut store, tab);
    select(&mut store, tab, first_id);
    let third = begin(&mut store, tab);
    let expected = [
        first_id,
        RepeaterHistoryEntryId::Live(third.summary.id),
        RepeaterHistoryEntryId::Live(second.summary.id),
    ];
    let page = store.repeater_history_page(tab, 0, 64).unwrap();
    assert_eq!(
        page.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(page.current_position, Some(0));
    let page = store.repeater_history_page(tab, 1, 1).unwrap();
    assert_eq!(page.total, 3);
    assert_eq!(page.rows[0].id, expected[1]);
    assert_eq!(page.next_offset, Some(2));
    assert!(
        store
            .repeater_history_page(tab, 3, 1)
            .unwrap()
            .rows
            .is_empty()
    );
    assert!(store.repeater_history_page(tab, 4, 1).is_err());
    assert!(store.repeater_history_page(tab, 0, 65).is_err());
    let selected = select(&mut store, tab, expected[2]);
    assert_eq!(selected.id, tab);
    assert_eq!(selected.draft.raw, RAW);
    assert_ne!(crate::request_to_raw(&second.request), RAW);
    let mut terminal = second;
    terminal.summary.revision += 1;
    terminal.summary.state = FlowState::Failed("controlled terminal failure".into());
    store.put(&terminal).unwrap();
    store.put(&terminal).unwrap();
    assert_eq!(store.repeater_history_page(tab, 0, 64).unwrap().total, 3);
    let saved = store.workspace().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("history.mio");
    let copy = directory.path().join("history-copy.mio");
    store.save(Some(&first_path)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&first_path, &copy] {
        let reopened = ProjectStore::open(path).unwrap();
        assert_eq!(reopened.workspace().unwrap(), saved);
        let page = reopened.repeater_history_page(tab, 0, 64).unwrap();
        assert_eq!(
            page.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(page.current_position, Some(2));
        assert_eq!(
            page.rows[2].diagnostic.as_deref(),
            Some("controlled terminal failure")
        );
        reopened.close().unwrap();
    }
}

#[test]
fn begin_failure_or_cancellation_rolls_back_pending_and_boundary_shift() {
    for cancel_during_shift in [false, true] {
        let (mut store, tab) = setup();
        let first = begin(&mut store, tab);
        select(
            &mut store,
            tab,
            RepeaterHistoryEntryId::Live(first.summary.id),
        );
        begin(&mut store, tab);
        let before = store.repeater_history_page(tab, 0, 64).unwrap();
        let workspace = store.workspace().unwrap();
        let draft = &workspace.tabs[0].draft;
        let rejected = pending(draft);
        let cancel = CancellationToken::new();
        if cancel_during_shift {
            let trigger = cancel.clone();
            store
                .connection
                .authorizer(Some(move |context: AuthContext<'_>| {
                    if matches!(
                        context.action,
                        AuthAction::Update {
                            table_name: "repeater_history",
                            column_name: "ordinal"
                        }
                    ) {
                        trigger.cancel();
                    }
                    Authorization::Allow
                }));
        } else {
            store.connection.execute_batch("CREATE TRIGGER fail_history BEFORE INSERT ON repeater_history BEGIN SELECT RAISE(ABORT,'controlled history insert failure'); END;").unwrap();
        }
        assert!(
            store
                .begin_repeater_send(tab, draft, &rejected, &cancel)
                .is_err()
        );
        store
            .connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        if !cancel_during_shift {
            store
                .connection
                .execute_batch("DROP TRIGGER fail_history;")
                .unwrap();
        }
        assert!(store.read_optional(rejected.summary.id).unwrap().is_none());
        assert_eq!(store.repeater_history_page(tab, 0, 64).unwrap(), before);
        assert_eq!(store.workspace().unwrap(), workspace);
    }
}

#[test]
fn navigation_guard_rejects_forged_raw_and_wrong_owner_but_allows_later_edits() {
    let (mut store, tab) = setup();
    let record = begin(&mut store, tab);
    let entry = RepeaterHistoryEntryId::Live(record.summary.id);
    let original = store.workspace().unwrap();
    let proposal = store
        .load_repeater_history(
            tab,
            entry,
            original.revision,
            original.tabs[0].draft.revision,
            &CancellationToken::new(),
        )
        .unwrap();
    let mut forged = original.clone();
    forged.revision += 1;
    forged.tabs[0] = proposal;
    forged.tabs[0].draft.raw.extend_from_slice(b"forged");
    assert!(store.save_workspace(&forged).is_err());
    assert_eq!(store.workspace().unwrap(), original);
    assert!(
        store
            .load_repeater_history(
                Uuid::new_v4(),
                entry,
                original.revision,
                1,
                &CancellationToken::new()
            )
            .is_err()
    );
    select(&mut store, tab, entry);
    let mut edited = store.workspace().unwrap();
    let cursor = edited.tabs[0].history.as_ref().unwrap().cursor.clone();
    edited.revision += 1;
    edited.tabs[0].draft.revision += 1;
    edited.tabs[0].draft.raw.extend_from_slice(b"editing");
    store.save_workspace(&edited).unwrap();
    assert_eq!(
        store.workspace().unwrap().tabs[0]
            .history
            .as_ref()
            .unwrap()
            .cursor,
        cursor
    );
    let reads_before = store.repeater_history_page(tab, 0, 1).unwrap();
    assert_eq!(reads_before.rows[0].id, entry);
}

#[test]
fn send_completion_binds_registered_revision_without_replacing_later_edits() {
    for (previous_selection, later_edit, terminal) in [
        (false, false, false),
        (true, false, true),
        (true, true, false),
    ] {
        let (mut store, tab) = setup();
        if previous_selection {
            let previous = begin(&mut store, tab);
            select(
                &mut store,
                tab,
                RepeaterHistoryEntryId::Live(previous.summary.id),
            );
        }
        let mut record = begin(&mut store, tab);
        if terminal {
            record.summary.revision += 1;
            record.summary.state = FlowState::Failed("controlled send failure".into());
            store.put(&record).unwrap();
        }
        let mut workspace = store.workspace().unwrap();
        let submitted = workspace.tabs[0].draft.clone();
        workspace.revision += 1;
        let tab = &mut workspace.tabs[0];
        tab.last_flow = Some(record.summary.id);
        tab.archive_response = None;
        tab.history
            .get_or_insert_with(RepeaterHistory::default)
            .cursor = Some(RepeaterHistoryCursor {
            entry: RepeaterHistoryEntryId::Live(record.summary.id),
            draft_revision: submitted.revision,
        });
        if later_edit {
            tab.draft.revision += 1;
            tab.draft.raw.extend_from_slice(b"editing");
            tab.draft.target = "http://127.0.0.1:18766".into();
            tab.draft.version = HttpVersion::Http2;
        }
        store.save_workspace(&workspace).unwrap();
        assert_eq!(store.workspace().unwrap(), workspace);
        assert_eq!(
            store
                .repeater_history_page(workspace.tabs[0].id, 0, 64)
                .unwrap()
                .current_position,
            Some(u64::from(previous_selection))
        );
    }
}

#[test]
fn send_completion_preserves_edits_saved_while_running_and_reopens_with_response() {
    for previous_selection in [false, true] {
        let (mut store, tab_id) = setup();
        if previous_selection {
            let previous = begin(&mut store, tab_id);
            select(
                &mut store,
                tab_id,
                RepeaterHistoryEntryId::Live(previous.summary.id),
            );
        }
        let mut record = begin(&mut store, tab_id);
        let mut edited = store.workspace().unwrap();
        let submitted = edited.tabs[0].draft.clone();
        edited.revision += 1;
        edited.tabs[0].draft.revision += 1;
        edited.tabs[0].draft.raw.extend_from_slice(b"saved edit");
        edited.tabs[0].draft.target = "http://127.0.0.1:18766".into();
        edited.tabs[0].draft.version = HttpVersion::Http2;
        store.save_workspace(&edited).unwrap();
        assert_eq!(store.workspace().unwrap(), edited);

        record.summary.revision += 1;
        record.summary.state = FlowState::Complete;
        record.summary.status = Some(201);
        record.summary.response_bytes = 3;
        record.response = Some(crate::HttpResponse {
            status: 201,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: Vec::new(),
            body: vec![255, 0, 128],
            trailers: Vec::new(),
        });
        store.put(&record).unwrap();
        let mut bound = edited.clone();
        bound.revision += 1;
        let tab = &mut bound.tabs[0];
        tab.last_flow = Some(record.summary.id);
        tab.archive_response = None;
        tab.history
            .get_or_insert_with(RepeaterHistory::default)
            .cursor = Some(RepeaterHistoryCursor {
            entry: RepeaterHistoryEntryId::Live(record.summary.id),
            draft_revision: submitted.revision,
        });
        store.save_workspace(&bound).unwrap();
        assert_eq!(store.workspace().unwrap(), bound);
        assert_eq!(bound.tabs[0].draft, edited.tabs[0].draft);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("edited-during-send.mio");
        store.save(Some(&path)).unwrap();
        store.close().unwrap();
        let reopened = ProjectStore::open(&path).unwrap();
        let restored = reopened.workspace().unwrap();
        assert_eq!(restored, bound);
        assert_eq!(
            reopened
                .read_optional(restored.tabs[0].last_flow.unwrap())
                .unwrap(),
            Some(record.clone())
        );
        let original = reopened
            .load_repeater_history(
                tab_id,
                RepeaterHistoryEntryId::Live(record.summary.id),
                restored.revision,
                restored.tabs[0].draft.revision,
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(original.draft.raw, submitted.raw);
        assert_eq!(original.draft.target, submitted.target);
        assert_eq!(original.draft.version, submitted.version);
        reopened.close().unwrap();
    }
}

#[test]
fn send_completion_rejects_stale_or_unregistered_draft_bindings() {
    for invalid in [
        "raw",
        "target",
        "version",
        "revision",
        "navigated",
        "wrong owner",
        "unregistered",
        "unversioned edit",
    ] {
        let (mut store, tab_id) = setup();
        let original = store.workspace().unwrap();
        let submitted = original.tabs[0].draft.clone();
        let record = if invalid == "unregistered" {
            let record = pending(&submitted);
            store.put(&record).unwrap();
            record
        } else {
            begin(&mut store, tab_id)
        };
        let entry = RepeaterHistoryEntryId::Live(record.summary.id);
        match invalid {
            "raw" => {
                store
                    .connection
                    .execute(
                        "UPDATE repeater_history SET raw=?1 WHERE flow=?2",
                        params![
                            b"different original".as_slice(),
                            record.summary.id.to_string()
                        ],
                    )
                    .unwrap();
            }
            "target" | "version" | "revision" => {
                let mut meta = DraftMeta {
                    revision: submitted.revision,
                    target: submitted.target.clone(),
                    version: submitted.version,
                    send_cursor: None,
                };
                match invalid {
                    "target" => meta.target = "http://127.0.0.1:18766".into(),
                    "version" => meta.version = HttpVersion::Http2,
                    "revision" => meta.revision += 1,
                    _ => unreachable!(),
                }
                store
                    .connection
                    .execute(
                        "UPDATE repeater_history SET draft_meta=?1 WHERE flow=?2",
                        params![
                            serde_json::to_vec(&meta).unwrap(),
                            record.summary.id.to_string()
                        ],
                    )
                    .unwrap();
            }
            "navigated" => {
                select(&mut store, tab_id, entry);
            }
            "wrong owner" => {
                store
                    .connection
                    .execute(
                        "UPDATE repeater_history SET tab=?1 WHERE flow=?2",
                        params![Uuid::new_v4().to_string(), record.summary.id.to_string()],
                    )
                    .unwrap();
            }
            "unregistered" | "unversioned edit" => {}
            _ => unreachable!(),
        }
        let before = store.workspace().unwrap();
        let mut proposed = before.clone();
        proposed.revision += 1;
        let tab = &mut proposed.tabs[0];
        tab.last_flow = Some(record.summary.id);
        tab.archive_response = None;
        tab.history
            .get_or_insert_with(RepeaterHistory::default)
            .cursor = Some(RepeaterHistoryCursor {
            entry,
            draft_revision: submitted.revision,
        });
        if invalid == "unversioned edit" {
            tab.draft.raw.extend_from_slice(b"editing");
        }
        assert!(store.save_workspace(&proposed).is_err(), "{invalid}");
        assert_eq!(store.workspace().unwrap(), before, "{invalid}");
    }
}
