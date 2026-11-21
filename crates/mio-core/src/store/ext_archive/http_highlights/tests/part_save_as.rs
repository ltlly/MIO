use super::*;

#[test]
fn schema_one_preserves_notes_and_keeps_unknown_source_highlight_explicit() {
    let (mut store, source, items) = fixture(3);
    let owner = project(&store);
    let note = store
        .set_ext_http_notes(
            owner,
            source,
            items[0].id,
            0,
            "schema22 note",
            &CancellationToken::new(),
        )
        .unwrap();
    for item in &items {
        metadata(&store, item, |value| {
            value["record"]["mapping"]["value"]
                .as_object_mut()
                .unwrap()
                .remove("highlight");
        });
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("schema-one-notes.mio");
    store.save(Some(&path)).unwrap();
    store.close().unwrap();
    let mut store = ProjectStore::open(&path).unwrap();
    assert_eq!(
        store
            .connection
            .pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        crate::store::SCHEMA
    );
    assert_eq!(
        store.ext_http_notes(owner, source, items[0].id).unwrap(),
        note
    );
    assert!(
        store
            .set_ext_http_highlights(
                owner,
                &[change(source, items[0].id, 0, 0)],
                &CancellationToken::new()
            )
            .is_err()
    );
    let (_, rows) = store.history_page(0, 10, "").unwrap();
    for row in rows {
        let HistoryRow::ExtProxy(row) = row else {
            unreachable!()
        };
        assert!(row.highlight.is_none() && row.highlight_error.is_some());
    }
    assert_eq!(store.read_source_range(source, 0, 1024).unwrap(), [0; 1024]);
    store.close().unwrap();
}
