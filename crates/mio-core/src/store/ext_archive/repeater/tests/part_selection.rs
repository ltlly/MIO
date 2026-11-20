use super::*;

#[test]
fn selected_raw_binary_headers_and_empty_target_survive_workspace_save_and_reopen() {
    let (mut store, source_id, item, message) = one();
    let cancel = CancellationToken::new();
    let mut workspace = store.workspace().unwrap();
    let tab = store
        .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
        .unwrap();
    let another = store
        .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
        .unwrap();
    assert_ne!(tab.id, another.id);
    assert_eq!(tab.title, "外部归档请求");
    assert_eq!(tab.group, None);
    assert_eq!(tab.last_flow, None);
    assert_eq!(tab.draft.revision, 1);
    assert_eq!(tab.draft.version, HttpVersion::Http11);
    assert!(tab.draft.target.is_empty());
    assert_eq!(tab.draft.raw, RAW);
    assert!(crate::parse_request(&tab.draft.raw, &tab.draft.target, tab.draft.version).is_err());
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(store.page(0, 1, "").unwrap().0, 0);

    workspace.active = Some(tab.id);
    workspace.tabs.push(tab);
    workspace.revision = 1;
    store.save_workspace(&workspace).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repeater.mio");
    store.save(Some(&path)).unwrap();
    store.close().unwrap();
    let reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(reopened.workspace().unwrap(), workspace);
    assert_eq!(
        reopened
            .ext_message_fragment(message.id, 0, RAW.len())
            .unwrap()
            .bytes,
        RAW
    );
    assert_eq!(reopened.page(0, 1, "").unwrap().0, 0);
    reopened.close().unwrap();
}

#[test]
fn only_the_selected_current_or_history_request_is_used_without_missing_or_empty_fallback() {
    let current = b"POST /current HTTP/2\r\n\r\ncurrent";
    let history = b"POST /history HTTP/1.1\r\n\r\nhistory";
    let archive = [current.as_slice(), history.as_slice()].concat();
    let (mut store, source_id) = pending(&archive);
    let mut missing = item(source_id, "root.f2.f1", 1);
    missing.metadata_json = br#"{"f4":{"state":"missing"}}"#.to_vec();
    let mut null = item(source_id, "root.f2.f1", 2);
    null.metadata_json = br#"{"f4":{"state":"null"}}"#.to_vec();
    let items = [
        item(source_id, "root.f2.f1", 0),
        item(source_id, "root.f2.f1[0].f2", 0),
        missing,
        null,
        item(source_id, "root.f2.f1", 3),
    ];
    let mut current_message = message(&items[0], 4, 0, current);
    current_message.role = "current_request".into();
    current_message.declared_version = Some(HttpVersion::Http2);
    current_message.request_target = Some("/current".into());
    let mut history_message = message(&items[1], 2, current.len() as u64, history);
    history_message.request_target = Some("/history".into());
    let mut empty_message = message(&items[4], 4, archive.len() as u64, &[]);
    empty_message.role = "current_request".into();
    empty_message.declared_version = None;
    empty_message.method = None;
    empty_message.request_target = None;
    let messages = [current_message, history_message, empty_message];
    finish(&mut store, source_id, &items, &messages);
    let workspace = Workspace::default();
    let cancel = CancellationToken::new();
    let current_tab = store
        .ext_repeater_tab(source_id, items[0].id, messages[0].id, &workspace, &cancel)
        .unwrap();
    assert_eq!(current_tab.draft.raw, current);
    assert_eq!(current_tab.draft.version, HttpVersion::Http2);
    assert!(current_tab.draft.target.is_empty());
    assert_eq!(
        store
            .ext_repeater_tab(source_id, items[1].id, messages[1].id, &workspace, &cancel)
            .unwrap()
            .draft
            .raw,
        history
    );
    let reads = deny_chunks(&store);
    for item in &items[2..4] {
        let error = store
            .ext_repeater_tab(source_id, item.id, messages[1].id, &workspace, &cancel)
            .unwrap_err();
        assert!(error.to_string().contains("不属于"));
        let error = store
            .ext_repeater_tab(source_id, item.id, Uuid::new_v4(), &workspace, &cancel)
            .unwrap_err();
        assert!(error.to_string().contains("不存在"));
    }
    let error = store
        .ext_repeater_tab(source_id, items[4].id, messages[2].id, &workspace, &cancel)
        .unwrap_err();
    assert!(error.to_string().contains("为空"));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(store.workspace().unwrap(), workspace);
}

#[test]
fn invalid_ownership_role_version_ranges_and_incomplete_source_do_not_read_chunks() {
    let (store, source_id, item, message) = one();
    let reads = deny_chunks(&store);
    let workspace = Workspace::default();
    let cancel = CancellationToken::new();
    let reject = |source, item, message, expected: &str| {
        let error = store
            .ext_repeater_tab(source, item, message, &workspace, &cancel)
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert_eq!(reads.load(Ordering::SeqCst), 0);
    };
    reject(Uuid::new_v4(), item.id, message.id, "不属于");
    reject(source_id, Uuid::new_v4(), message.id, "不属于");
    reject(source_id, item.id, Uuid::new_v4(), "不存在");

    let mut changed = message.clone();
    changed.item_id = Uuid::new_v4();
    replace_summary(&store, &changed);
    reject(source_id, item.id, message.id, "归属不一致");
    for role in ["response", "unmapped"] {
        changed = message.clone();
        changed.role = role.into();
        replace_summary(&store, &changed);
        reject(source_id, item.id, message.id, "明确映射");
    }
    changed = message.clone();
    changed.declared_version = None;
    replace_summary(&store, &changed);
    reject(source_id, item.id, message.id, "声明版本");
    changed = message.clone();
    changed.raw_offset = u64::MAX;
    replace_summary(&store, &changed);
    reject(source_id, item.id, message.id, "读取范围");
    changed = message.clone();
    changed.raw_bytes = 0;
    changed.body_offset = None;
    replace_summary(&store, &changed);
    reject(source_id, item.id, message.id, "为空");
    replace_summary(&store, &message);
    store
        .connection
        .execute("UPDATE ext_sources SET complete=0", [])
        .unwrap();
    reject(source_id, item.id, message.id, "尚未完成");
    store
        .connection
        .execute("UPDATE ext_sources SET complete=1", [])
        .unwrap();
    assert!(
        store
            .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
            .is_err()
    );
    assert!(
        reads.load(Ordering::SeqCst) > 0,
        "the raw-read guard must be active"
    );
    assert_eq!(store.workspace().unwrap(), workspace);
}

#[test]
fn oversized_summary_is_rejected_before_allocating_or_reading_its_claimed_archive() {
    let (store, source_id, item, mut message) = one();
    let cancel = CancellationToken::new();
    message.raw_bytes = (MAX_BODY + 256 * 1024 + 1) as u64;
    // A hostile index claims a large complete archive; no large payload fixture is necessary.
    store
        .connection
        .execute(
            "UPDATE ext_sources SET source_bytes=?1,written_bytes=?1,next_chunk=?2",
            params![
                message.raw_bytes,
                message.raw_bytes.div_ceil(EXT_CHUNK_BYTES as u64)
            ],
        )
        .unwrap();
    replace_summary(&store, &message);
    let reads = deny_chunks(&store);
    let error = store
        .ext_repeater_tab(
            source_id,
            item.id,
            message.id,
            &Workspace::default(),
            &cancel,
        )
        .unwrap_err();
    assert!(error.to_string().contains("可编辑大小限额"), "{error}");
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(store.workspace().unwrap(), Workspace::default());
}

#[test]
fn invalid_workspace_and_combined_open_closed_tab_limit_fail_before_raw_reads() {
    let (store, source_id, item, message) = one();
    let reads = deny_chunks(&store);
    let cancel = CancellationToken::new();
    let mut workspace = Workspace {
        active: Some(Uuid::new_v4()),
        ..Workspace::default()
    };
    let error = store
        .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
        .unwrap_err();
    assert!(error.to_string().contains("当前工作标签不存在"));
    workspace.active = None;
    for index in 0..512 {
        let tab = WorkTab::from_request(None, index.to_string());
        if index % 2 == 0 {
            workspace.tabs.push(tab);
        } else {
            workspace.closed.push(tab);
        }
    }
    workspace.validate().unwrap();
    let error = store
        .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
        .unwrap_err();
    assert!(error.to_string().contains("512"));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(workspace.tabs.len(), 256);
    assert_eq!(workspace.closed.len(), 256);
    let intruder = intruder_draft(workspace.closed.pop().unwrap());
    workspace.intruder.closed.push(intruder);
    let error = store
        .ext_repeater_tab(source_id, item.id, message.id, &workspace, &cancel)
        .unwrap_err();
    assert!(error.to_string().contains("512"));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
}
