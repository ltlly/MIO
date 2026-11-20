use super::*;

#[test]
fn total_raw_limit_includes_closed_tabs_and_accepts_exactly_64_mib() {
    let (store, source_id, item, message) = one();
    let mut workspace = Workspace::default();
    let cancel = CancellationToken::new();
    for index in 0..4 {
        let mut tab = WorkTab::from_request(None, index.to_string());
        tab.draft.raw = vec![0; MAX_BODY];
        if index == 3 {
            tab.draft.raw.truncate(MAX_BODY - RAW.len());
        }
        if index < 2 {
            workspace.tabs.push(tab);
        } else {
            workspace.closed.push(tab);
        }
    }
    assert_eq!(
        store
            .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
            .unwrap()
            .draft
            .raw,
        RAW
    );
    workspace.closed.last_mut().unwrap().draft.raw.push(0);
    workspace.validate().unwrap();
    let reads = deny_chunks(&store);
    let error = store
        .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
        .unwrap_err();
    assert!(error.to_string().contains("64 MiB"));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(workspace.tabs.len(), 2);
    assert_eq!(workspace.closed.len(), 2);
    workspace.closed.last_mut().unwrap().draft.raw.pop();
    let mut intruder = intruder_draft(WorkTab::from_request(None, "Intruder".into()));
    intruder.spec.draft.raw.clear();
    intruder.spec.payload_sets = vec![vec![vec![255]]];
    workspace.intruder.closed.push(intruder);
    workspace.validate().unwrap();
    let error = store
        .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
        .unwrap_err();
    assert!(error.to_string().contains("64 MiB"));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
}

#[test]
fn chunk_boundaries_read_each_block_once_and_missing_blocks_never_return_partial_drafts() {
    let mut request = b"POST /fixture HTTP/1.1\r\nX-A: one\r\nX-A: two\r\n\r\n".to_vec();
    request.extend((0..EXT_CHUNK_BYTES).map(|index| (index % 251) as u8));
    let start = EXT_CHUNK_BYTES - 7;
    let mut archive = vec![0; start];
    archive.extend_from_slice(&request);
    archive.extend_from_slice(b"not part of the request");
    let (mut store, source_id) = pending(&archive);
    let item = item(source_id, "root.f1.f0", 0);
    let message = message(&item, 15, start as u64, &request);
    finish(
        &mut store,
        source_id,
        std::slice::from_ref(&item),
        std::slice::from_ref(&message),
    );
    let reads = Arc::new(AtomicUsize::new(0));
    let observed = reads.clone();
    store
        .connection
        .authorizer(Some(move |context: AuthContext<'_>| {
            if let AuthAction::Read {
                table_name: "ext_source_chunks",
                column_name: "raw_bytes",
                ..
            } = context.action
            {
                observed.fetch_add(1, Ordering::SeqCst);
            }
            Authorization::Allow
        }));
    let workspace = Workspace::default();
    let cancel = CancellationToken::new();
    let tab = store
        .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
        .unwrap();
    assert_eq!(tab.draft.raw, request);
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    store
        .connection
        .execute("DELETE FROM ext_source_chunks WHERE ordinal=1", [])
        .unwrap();
    let error = store
        .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
        .unwrap_err();
    assert!(error.to_string().contains("缺块"));
    assert_eq!(store.workspace().unwrap(), workspace);
}

#[test]
fn cancellation_stops_before_more_chunks_without_returning_or_saving_a_draft() {
    let start = EXT_CHUNK_BYTES - 7;
    let mut archive = vec![0; start];
    archive.extend_from_slice(RAW);
    let (mut store, source_id) = pending(&archive);
    let item = item(source_id, "root.f1.f0", 0);
    let message = message(&item, 15, start as u64, RAW);
    finish(
        &mut store,
        source_id,
        std::slice::from_ref(&item),
        std::slice::from_ref(&message),
    );
    // The read loop must observe cancellation independently of SQLite's progress callback.
    store.connection.progress_handler(0, None::<fn() -> bool>);
    let workspace = store.workspace().unwrap();
    for pre_cancelled in [false, true] {
        let cancel = CancellationToken::new();
        if pre_cancelled {
            cancel.cancel();
        }
        let reads = Arc::new(AtomicUsize::new(0));
        let observed = reads.clone();
        let trigger = cancel.clone();
        store
            .connection
            .authorizer(Some(move |context: AuthContext<'_>| {
                if let AuthAction::Read {
                    table_name: "ext_source_chunks",
                    column_name: "raw_bytes",
                    ..
                } = context.action
                    && observed.fetch_add(1, Ordering::SeqCst) == 0
                {
                    trigger.cancel();
                }
                Authorization::Allow
            }));
        let result = store.ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel);
        assert!(cancel.is_cancelled());
        assert_eq!(store.workspace().unwrap(), workspace);
        assert_eq!(
            reads.load(Ordering::SeqCst),
            usize::from(!pre_cancelled),
            "pre_cancelled={pre_cancelled}; returned_draft={}",
            result.is_ok()
        );
        let error = result.expect_err("cancelled archive reads must not return a draft");
        assert!(error.to_string().contains("取消"), "{error}");
    }
}
