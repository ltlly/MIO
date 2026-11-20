use super::*;

#[test]
fn atomic_batches_reject_duplicate_alias_owners_and_roll_back_earlier_writes() {
    let (mut store, source, items) = fixture("original");
    let change = |item, expected, text: &str| ExtHttpNotesChange {
        source_id: source,
        item_id: item,
        expected_notes_revision: expected,
        text: text.into(),
    };
    let cancel = CancellationToken::new();
    {
        let tx = store.connection.transaction().unwrap();
        assert!(
            write_changes(
                &tx,
                &[change(items[0].id, 0, "a"), change(items[2].id, 0, "a")],
                &cancel
            )
            .is_err()
        );
        assert_eq!(
            tx.query_row::<usize, _, _>("SELECT count(*) FROM ext_http_notes", [], |row| row
                .get(0))
                .unwrap(),
            0
        );
    }
    {
        let tx = store.connection.transaction().unwrap();
        assert!(
            write_changes(
                &tx,
                &[change(items[0].id, 0, "a"), change(items[1].id, 9, "b")],
                &cancel
            )
            .is_err()
        );
    }
    assert_eq!(
        store
            .ext_http_notes(project(&store), source, items[0].id)
            .unwrap()
            .revision,
        0
    );
    {
        let tx = store.connection.transaction().unwrap();
        let changes = [
            change(items[2].id, 0, "first"),
            change(items[3].id, 0, "second"),
        ];
        assert_eq!(
            write_changes(&tx, &changes, &cancel).unwrap(),
            [(source, items[2].id, 1), (source, items[3].id, 1)]
        );
        tx.commit().unwrap();
    }
    assert_eq!(
        store
            .ext_http_notes(project(&store), source, items[0].id)
            .unwrap()
            .text,
        "first"
    );
    assert_eq!(
        store
            .ext_http_notes(project(&store), source, items[1].id)
            .unwrap()
            .text,
        "second"
    );
}
