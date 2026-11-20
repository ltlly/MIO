use super::*;

#[test]
fn complete_archive_indices_reports_and_binary_fragments_survive_save_as_and_reopen() {
    let mut raw = vec![b'A'; EXT_CHUNK_BYTES + 1];
    let http = b"GET / HTTP/1.1\r\n\r\n\0\xff";
    let start = raw.len() - http.len();
    raw[start..].copy_from_slice(http);
    let (mut store, source_id) = archived(&raw);
    let first = item(source_id, 0);
    let mut absent = item(source_id, 1);
    absent.state = "null".into();
    absent.object_address = None;
    absent.metadata_json = br#"{"slot":"null"}"#.to_vec();
    let mut failed_history = item(source_id, 0);
    failed_history.collection = "root.f2.f1[0].f2".into();
    failed_history.state = "failed".into();
    failed_history.metadata_json = br#"{"slot":"failed","kind":"unverified field"}"#.to_vec();
    let mut summary = message(&first, 15);
    summary.raw_offset = start as u64;
    summary.raw_bytes = http.len() as u64;
    summary.body_offset = Some(http.len() as u64 - 2);
    summary.request_target = Some("/".into());
    let items = [first, absent, failed_history];
    store
        .put_ext_batch(&items, std::slice::from_ref(&summary))
        .unwrap();
    let report = final_report(raw.len(), &items, std::slice::from_ref(&summary));
    store.finish_source(source_id, &report).unwrap();
    store.finish_source(source_id, &report).unwrap();
    assert_eq!(store.ext_report(source_id).unwrap(), report);
    assert_eq!(
        store.ext_sources().unwrap(),
        [ExtSourceInfo {
            id: source_id,
            report: report.clone()
        }]
    );
    let codecs: Vec<u8> = store
        .connection
        .prepare("SELECT codec FROM ext_source_chunks ORDER BY ordinal")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        codecs,
        [1, 0],
        "compressed and raw blocks coexist without a whole-file decode"
    );
    for offset in (0..raw.len()).step_by(EXT_FRAGMENT_BYTES) {
        let count = (raw.len() - offset).min(EXT_FRAGMENT_BYTES);
        assert_eq!(
            store
                .read_source_range(source_id, offset as u64, count)
                .unwrap(),
            raw[offset..offset + count]
        );
    }
    assert_eq!(
        store
            .read_source_range(source_id, EXT_CHUNK_BYTES as u64 - 4, 5)
            .unwrap(),
        raw[EXT_CHUNK_BYTES - 4..]
    );
    assert_eq!(
        store
            .ext_message_fragment(summary.id, 0, http.len())
            .unwrap(),
        ExtMessageFragment {
            summary: summary.clone(),
            offset: 0,
            bytes: http.to_vec(),
        }
    );
    assert_eq!(store.page(0, 1, "").unwrap().0, 0);
    let http_bodies: u64 = store
        .connection
        .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        http_bodies, 0,
        "archive bodies must not be duplicated into live flows"
    );

    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("imported.mio");
    let destination = directory.path().join("copy.mio");
    let (_, total_bytes) = store.save(Some(&source)).unwrap();
    assert!(total_bytes >= fs::metadata(&source).unwrap().len());
    let mut workspace = store.workspace().unwrap();
    workspace.open_request(None).unwrap();
    store
        .save_with(
            None,
            Some(&workspace),
            &[],
            &[],
            &[],
            &CancellationToken::new(),
        )
        .unwrap();
    store.checkpoint().unwrap();
    let before = fs::read(&source).unwrap();
    store.save(Some(&destination)).unwrap();
    assert_eq!(fs::read(&source).unwrap(), before);
    store.close().unwrap();
    for path in [&source, &destination] {
        let store = ProjectStore::open(path).unwrap();
        assert_eq!(store.workspace().unwrap(), workspace);
        assert_eq!(store.ext_report(source_id).unwrap(), report);
        assert_eq!(
            store.ext_message_page(source_id, None, 0, 1).unwrap(),
            ExtMessagePage {
                total: 1,
                rows: vec![summary.clone()],
                next_offset: None,
            }
        );
        assert_eq!(
            store
                .ext_message_fragment(summary.id, http.len() as u64 - 2, 2)
                .unwrap()
                .bytes,
            [0, 255]
        );
        for item in &items {
            assert_eq!(store.ext_item(item.id).unwrap(), *item);
        }
        store.close().unwrap();
    }
}

#[test]
fn archive_and_index_batches_roll_back_together_and_retries_do_not_replace_data() {
    let length = EXT_CHUNK_BYTES + 2;
    let mut store = ProjectStore::create(None).unwrap();
    let id = Uuid::new_v4();
    store.begin_source(id, &initial(length)).unwrap();
    let full = vec![b'Q'; EXT_CHUNK_BYTES];
    assert!(
        store
            .append_source_chunks(id, 0, &[full.clone(), vec![1]])
            .is_err()
    );
    assert_eq!(
        source_state(&store.connection, id).unwrap().written_bytes,
        0
    );
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM ext_source_chunks", [], |row| row
                .get::<_, usize>(
                0
            ))
            .unwrap(),
        0
    );
    assert!(store.append_source_chunk(id, 1, &[1, 2]).is_err());
    store
        .append_source_chunks(id, 0, &[full.clone(), vec![1, 2]])
        .unwrap();
    store.append_source_chunk(id, 0, &full).unwrap();
    assert!(store.append_source_chunk(id, 1, &[2, 1]).is_err());
    assert_eq!(
        read_chunk(&store.connection, id, length as u64, 1).unwrap(),
        [1, 2]
    );

    let original = item(id, 0);
    store.put_ext_item(&original).unwrap();
    let added = item(id, 1);
    let first_message = message(&original, 15);
    let mut failed = message(&added, 18);
    failed.item_id = Uuid::new_v4();
    assert!(
        store
            .put_ext_batch(
                std::slice::from_ref(&added),
                &[first_message.clone(), failed]
            )
            .is_err()
    );
    assert!(store.ext_item(added.id).is_err());
    assert!(read_message_summary(&store.connection, first_message.id).is_err());
    assert_eq!(store.ext_item(original.id).unwrap(), original);
    let second_message = message(&added, 18);
    let items = [original.clone(), added.clone()];
    let messages = [first_message.clone(), second_message.clone()];
    store.put_ext_batch(&items, &messages).unwrap();
    store.put_ext_batch(&items, &messages).unwrap();
    let mut changed = second_message.clone();
    changed.request_target = Some("/other".into());
    assert!(store.put_ext_message(&changed).is_err());
    changed = second_message.clone();
    changed.id = Uuid::new_v4();
    assert!(store.put_ext_message(&changed).is_err());
    let mut changed = added.clone();
    changed.state = "changed".into();
    assert!(store.put_ext_item(&changed).is_err());
    changed = added.clone();
    changed.id = Uuid::new_v4();
    assert!(store.put_ext_item(&changed).is_err());
    assert_eq!(store.ext_item(added.id).unwrap(), added);
    assert_eq!(
        read_message_summary(&store.connection, second_message.id).unwrap(),
        second_message
    );

    let mut report = final_report(length, &items, &messages);
    report.collections[0].indexed_items -= 1;
    report.indexed_items -= 1;
    assert!(store.finish_source(id, &report).is_err());
    assert!(!store.ext_report(id).unwrap().complete_archive);
    let report = final_report(length, &items, &messages);
    store.finish_source(id, &report).unwrap();
    assert!(store.append_source_chunk(id, 0, &full).is_err());
    assert!(store.put_ext_item(&original).is_err());
    assert!(store.put_ext_message(&first_message).is_err());
    assert!(store.begin_source(id, &initial(length)).is_err());
    let mut changed_report = report.clone();
    changed_report.gaps.clear();
    assert!(store.finish_source(id, &changed_report).is_err());
    assert_eq!(store.ext_report(id).unwrap(), report);
}
